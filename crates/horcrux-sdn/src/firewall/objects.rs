//! Firewall objects (Proxmox "IP sets", "aliases", "macros") and the host
//! default-policy options. Rules refer to them from `source` / `dest`:
//!
//! - `+name` an IP set: a list of addresses/CIDRs (a set with no entries
//!   matches nothing, so a rule that uses it is not installed);
//! - `name` an alias: one named address/CIDR;
//! - `dport = "macro:SSH"` a macro: protocol + port pairs of a well known
//!   service, installed as one rule per pair.
//!
//! Objects are expanded when the nft rule is built, so changing an IP set
//! and re-applying changes what the kernel matches.
use horcrux_common::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::Ipv4Addr;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct IpSet {
    pub name: String,
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub entries: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Alias {
    pub name: String,
    pub cidr: String,
    #[serde(default)]
    pub comment: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FwOptions {
    /// `accept` or `drop` for host input / output not matched by a rule.
    pub policy_in: String,
    pub policy_out: String,
}

impl Default for FwOptions {
    fn default() -> Self {
        Self {
            policy_in: "accept".into(),
            policy_out: "accept".into(),
        }
    }
}

impl FwOptions {
    pub fn validate(&self) -> Result<()> {
        for (what, v) in [
            ("policy_in", &self.policy_in),
            ("policy_out", &self.policy_out),
        ] {
            if v != "accept" && v != "drop" {
                return Err(Error::InvalidConfig(format!(
                    "{what} must be accept or drop"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct FwObjects {
    pub ipsets: BTreeMap<String, IpSet>,
    pub aliases: BTreeMap<String, Alias>,
    pub options: FwOptions,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MacroInfo {
    pub name: &'static str,
    pub description: &'static str,
    /// `(protocol, port)` pairs.
    pub rules: &'static [(&'static str, &'static str)],
}

pub const MACROS: &[MacroInfo] = &[
    MacroInfo {
        name: "SSH",
        description: "Secure Shell",
        rules: &[("tcp", "22")],
    },
    MacroInfo {
        name: "HTTP",
        description: "Web",
        rules: &[("tcp", "80")],
    },
    MacroInfo {
        name: "HTTPS",
        description: "Secure web",
        rules: &[("tcp", "443")],
    },
    MacroInfo {
        name: "DNS",
        description: "Domain name service",
        rules: &[("udp", "53"), ("tcp", "53")],
    },
    MacroInfo {
        name: "NTP",
        description: "Network time",
        rules: &[("udp", "123")],
    },
    MacroInfo {
        name: "SMTP",
        description: "Mail submission",
        rules: &[("tcp", "25"), ("tcp", "587")],
    },
    MacroInfo {
        name: "IMAP",
        description: "Mail retrieval",
        rules: &[("tcp", "143"), ("tcp", "993")],
    },
    MacroInfo {
        name: "POP3",
        description: "Mail retrieval (POP)",
        rules: &[("tcp", "110"), ("tcp", "995")],
    },
    MacroInfo {
        name: "FTP",
        description: "File transfer",
        rules: &[("tcp", "21")],
    },
    MacroInfo {
        name: "RDP",
        description: "Remote Desktop",
        rules: &[("tcp", "3389"), ("udp", "3389")],
    },
    MacroInfo {
        name: "VNC",
        description: "VNC displays 0-9",
        rules: &[("tcp", "5900-5909")],
    },
    MacroInfo {
        name: "MySQL",
        description: "MySQL / MariaDB",
        rules: &[("tcp", "3306")],
    },
    MacroInfo {
        name: "PostgreSQL",
        description: "PostgreSQL",
        rules: &[("tcp", "5432")],
    },
    MacroInfo {
        name: "Ping",
        description: "ICMP echo",
        rules: &[("icmp", "")],
    },
    MacroInfo {
        name: "SMB",
        description: "Windows file sharing",
        rules: &[("tcp", "445"), ("tcp", "139"), ("udp", "137-138")],
    },
    MacroInfo {
        name: "NFS",
        description: "NFS",
        rules: &[
            ("tcp", "2049"),
            ("udp", "2049"),
            ("tcp", "111"),
            ("udp", "111"),
        ],
    },
    MacroInfo {
        name: "Horcrux",
        description: "Horcrux web UI / API",
        rules: &[("tcp", "8006")],
    },
];

pub fn find_macro(name: &str) -> Option<&'static MacroInfo> {
    MACROS.iter().find(|m| m.name.eq_ignore_ascii_case(name))
}

/// `macro:SSH` -> the macro, if the port field names one.
pub fn macro_of(dport: &Option<String>) -> Option<&str> {
    dport.as_deref()?.trim().strip_prefix("macro:")
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// An IPv4 address or CIDR.
pub fn valid_cidr(s: &str) -> bool {
    let (ip, prefix) = match s.split_once('/') {
        Some((a, p)) => (a, Some(p)),
        None => (s, None),
    };
    ip.parse::<Ipv4Addr>().is_ok() && prefix.is_none_or(|p| p.parse::<u8>().is_ok_and(|p| p <= 32))
}

impl FwObjects {
    /// Check that `source` / `dest` / `dport` of a rule refer to something
    /// that exists (so a typo is an error when the rule is added, not a rule
    /// that silently never matches).
    pub fn check_rule_refs(
        &self,
        source: &Option<String>,
        dest: &Option<String>,
        dport: &Option<String>,
    ) -> Result<()> {
        for a in [source, dest].into_iter().flatten() {
            let a = a.trim();
            if let Some(set) = a.strip_prefix('+') {
                if !self.ipsets.contains_key(set) {
                    return Err(Error::InvalidConfig(format!("IP set {set} does not exist")));
                }
            } else if valid_name(a) && a != "any" && !self.aliases.contains_key(a) {
                return Err(Error::InvalidConfig(format!(
                    "alias {a} does not exist (use an address, +ipset or an alias)"
                )));
            }
        }
        if let Some(m) = macro_of(dport) {
            if find_macro(m).is_none() {
                return Err(Error::InvalidConfig(format!("unknown macro {m}")));
            }
        }
        Ok(())
    }

    /// The nft address match operand for a source/dest value: `None` =
    /// matches nothing (empty IP set), `Some(String)` = the operand text.
    pub fn resolve_addr(&self, a: &str) -> Option<String> {
        let a = a.trim();
        if let Some(set) = a.strip_prefix('+') {
            let set = self.ipsets.get(set)?;
            let e: Vec<&str> = set
                .entries
                .iter()
                .map(String::as_str)
                .filter(|e| valid_cidr(e))
                .collect();
            return match e.len() {
                0 => None,
                1 => Some(e[0].to_string()),
                _ => Some(format!("{{ {} }}", e.join(", "))),
            };
        }
        if let Some(al) = self.aliases.get(a) {
            return Some(al.cidr.clone());
        }
        Some(a.to_string())
    }

    pub fn ipset_in_use(
        &self,
        name: &str,
        rules: &[(&str, &Option<String>, &Option<String>)],
    ) -> Option<String> {
        let needle = format!("+{name}");
        rules
            .iter()
            .find(|(_, s, d)| {
                s.as_deref().map(str::trim) == Some(&needle)
                    || d.as_deref().map(str::trim) == Some(&needle)
            })
            .map(|(scope, _, _)| scope.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn objs() -> FwObjects {
        let mut o = FwObjects::default();
        o.ipsets.insert(
            "office".into(),
            IpSet {
                name: "office".into(),
                comment: String::new(),
                entries: vec!["10.1.0.0/16".into(), "192.168.1.5".into(), "bogus".into()],
            },
        );
        o.ipsets.insert(
            "empty".into(),
            IpSet {
                name: "empty".into(),
                ..Default::default()
            },
        );
        o.aliases.insert(
            "web1".into(),
            Alias {
                name: "web1".into(),
                cidr: "10.9.9.9".into(),
                comment: String::new(),
            },
        );
        o
    }

    #[test]
    fn expands_sets_aliases_and_plain_addresses() {
        let o = objs();
        assert_eq!(
            o.resolve_addr("+office").unwrap(),
            "{ 10.1.0.0/16, 192.168.1.5 }"
        );
        assert_eq!(o.resolve_addr("+empty"), None);
        assert_eq!(o.resolve_addr("web1").unwrap(), "10.9.9.9");
        assert_eq!(o.resolve_addr("172.16.0.0/12").unwrap(), "172.16.0.0/12");
    }

    #[test]
    fn references_are_checked() {
        let o = objs();
        let s = |x: &str| Some(x.to_string());
        assert!(o.check_rule_refs(&s("+office"), &None, &None).is_ok());
        assert!(o.check_rule_refs(&s("+nope"), &None, &None).is_err());
        assert!(o
            .check_rule_refs(&None, &s("web1"), &s("macro:ssh"))
            .is_ok());
        assert!(o.check_rule_refs(&None, &s("web2"), &None).is_err());
        assert!(o.check_rule_refs(&None, &None, &s("macro:Nope")).is_err());
        assert!(o
            .check_rule_refs(&s("10.0.0.0/8"), &s("any"), &None)
            .is_ok());
    }

    #[test]
    fn validators() {
        assert!(valid_cidr("10.0.0.1") && valid_cidr("10.0.0.0/8"));
        assert!(!valid_cidr("10.0.0.0/33") && !valid_cidr("x") && !valid_cidr("10.0.0.0/8; drop"));
        assert!(valid_name("web-1") && !valid_name("1web") && !valid_name("a b"));
        assert!(FwOptions {
            policy_in: "reject".into(),
            ..Default::default()
        }
        .validate()
        .is_err());
    }
}
