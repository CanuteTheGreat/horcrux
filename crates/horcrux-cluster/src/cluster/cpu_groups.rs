//! Named CPU compatibility groups (area 1 of the mixed-hardware cluster work).
//!
//! A node's group is `<vendor>-<level>` where level is the x86-64
//! microarchitecture level (v1..v4) derived from its CPU flags. A running guest
//! may only be live-migrated within one vendor, and only to a node whose level
//! is >= the source's level; or, for guests pinned to a baseline level, to any
//! node at or above that baseline. Pure functions, no I/O.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const V2: &[&str] = &["cx16", "lahf_lm", "popcnt", "sse4_1", "sse4_2", "ssse3"];
const V3: &[&str] = &[
    "avx", "avx2", "bmi1", "bmi2", "f16c", "fma", "abm", "movbe", "xsave",
];
const V4: &[&str] = &["avx512f", "avx512bw", "avx512cd", "avx512dq", "avx512vl"];

/// x86-64 microarchitecture level (1 = baseline, 4 = AVX-512).
pub fn x86_level(flags: &BTreeSet<String>) -> u8 {
    let has = |set: &[&str]| set.iter().all(|f| flags.contains(*f));
    if !has(V2) {
        return 1;
    }
    if !has(V3) {
        return 2;
    }
    if !has(V4) {
        return 3;
    }
    4
}

/// Parse the first `flags` line of /proc/cpuinfo text.
pub fn parse_flags(cpuinfo: &str) -> BTreeSet<String> {
    cpuinfo
        .lines()
        .find(|l| l.starts_with("flags"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuGroup {
    /// "intel", "amd" or "" (unknown).
    pub vendor: String,
    pub level: u8,
}

impl CpuGroup {
    pub fn from_cpuinfo(vendor: &str, cpuinfo: &str) -> Self {
        CpuGroup {
            vendor: vendor.to_string(),
            level: x86_level(&parse_flags(cpuinfo)),
        }
    }

    pub fn name(&self) -> String {
        let v = if self.vendor.is_empty() {
            "unknown"
        } else {
            &self.vendor
        };
        format!("{v}-v{}", self.level)
    }
}

/// Why a running guest on `src` cannot be live-migrated to `dst`
/// (None = allowed). Unknown vendor or unknown level (0) is permissive, like
/// the other gates.
pub fn live_migration_block(src: &CpuGroup, dst: &CpuGroup) -> Option<String> {
    if !src.vendor.is_empty() && !dst.vendor.is_empty() && src.vendor != dst.vendor {
        return Some(format!(
            "CPU vendor differs ({} -> {}): cold migrate only",
            src.name(),
            dst.name()
        ));
    }
    if src.level != 0 && dst.level != 0 && dst.level < src.level {
        return Some(format!(
            "target CPU level {} is older than source {}: cold migrate only",
            dst.name(),
            src.name()
        ));
    }
    None
}

/// One row of the "why can't this running VM go there" table shown in the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetVerdict {
    pub node: String,
    pub group: String,
    pub live_migration_allowed: bool,
    /// Why live migration is blocked (None when allowed).
    pub reason: Option<String>,
}

/// Live-migration verdict for every candidate target of a guest running on
/// `src`. The source node itself is skipped.
pub fn target_verdicts(
    src_name: &str,
    src: &CpuGroup,
    nodes: &[(String, CpuGroup)],
) -> Vec<TargetVerdict> {
    nodes
        .iter()
        .filter(|(n, _)| n != src_name)
        .map(|(n, g)| {
            let reason = live_migration_block(src, g);
            TargetVerdict {
                node: n.clone(),
                group: g.name(),
                live_migration_allowed: reason.is_none(),
                reason,
            }
        })
        .collect()
}

/// Highest baseline level every node in `groups` supports within one vendor
/// (the level a cluster-wide CPU model can safely use). None if vendors mix or
/// the list is empty.
pub fn common_baseline(groups: &[CpuGroup]) -> Option<CpuGroup> {
    let first = groups.first()?;
    if groups.iter().any(|g| g.vendor != first.vendor) {
        return None;
    }
    let level = groups.iter().map(|g| g.level).min()?;
    Some(CpuGroup {
        vendor: first.vendor.clone(),
        level,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(sets: &[&[&str]]) -> BTreeSet<String> {
        sets.iter()
            .flat_map(|s| s.iter().map(|f| f.to_string()))
            .collect()
    }

    #[test]
    fn target_verdicts_explain_blocks() {
        let g = |v: &str, l| CpuGroup {
            vendor: v.into(),
            level: l,
        };
        let nodes = vec![
            ("r6525".to_string(), g("amd", 3)),
            ("r430".to_string(), g("intel", 3)),
            ("old".to_string(), g("intel", 1)),
            ("r6525b".to_string(), g("amd", 3)),
        ];
        let v = target_verdicts("r6525", &g("amd", 3), &nodes);
        assert_eq!(v.len(), 3);
        assert!(!v[0].live_migration_allowed && v[0].reason.as_ref().unwrap().contains("vendor"));
        assert!(!v[1].live_migration_allowed);
        assert!(v[2].live_migration_allowed && v[2].reason.is_none());
    }

    #[test]
    fn levels_from_flags() {
        assert_eq!(x86_level(&flags(&[&["sse2", "ssse3"]])), 1);
        assert_eq!(x86_level(&flags(&[V2])), 2);
        assert_eq!(x86_level(&flags(&[V2, V3])), 3);
        assert_eq!(x86_level(&flags(&[V2, V3, V4])), 4);
        // Missing one v3 flag drops to v2.
        let mut f = flags(&[V2, V3]);
        f.remove("movbe");
        assert_eq!(x86_level(&f), 2);
    }

    #[test]
    fn parses_cpuinfo_flags() {
        let txt = "processor : 0\nflags\t\t: fpu sse2 ssse3\nmodel : 1\n";
        let f = parse_flags(txt);
        assert!(f.contains("sse2") && f.contains("ssse3") && f.len() == 3);
        assert!(parse_flags("nothing here").is_empty());
    }

    #[test]
    fn group_names() {
        let g = CpuGroup {
            vendor: "amd".into(),
            level: 3,
        };
        assert_eq!(g.name(), "amd-v3");
        let u = CpuGroup {
            vendor: String::new(),
            level: 1,
        };
        assert_eq!(u.name(), "unknown-v1");
    }

    #[test]
    fn migration_rules() {
        let epyc = CpuGroup {
            vendor: "amd".into(),
            level: 3,
        };
        let epyc4 = CpuGroup {
            vendor: "amd".into(),
            level: 4,
        };
        let xeon5000 = CpuGroup {
            vendor: "intel".into(),
            level: 1,
        };
        let e5 = CpuGroup {
            vendor: "intel".into(),
            level: 3,
        };
        assert!(live_migration_block(&epyc, &e5).unwrap().contains("vendor"));
        assert!(live_migration_block(&e5, &xeon5000)
            .unwrap()
            .contains("older"));
        assert!(live_migration_block(&xeon5000, &e5).is_none());
        assert!(live_migration_block(&epyc, &epyc4).is_none());
        assert!(live_migration_block(&epyc4, &epyc).is_some());
        let unk = CpuGroup {
            vendor: String::new(),
            level: 3,
        };
        assert!(live_migration_block(&unk, &epyc).is_none());
    }

    #[test]
    fn common_baseline_picks_lowest_within_vendor() {
        let a = CpuGroup {
            vendor: "intel".into(),
            level: 3,
        };
        let b = CpuGroup {
            vendor: "intel".into(),
            level: 1,
        };
        assert_eq!(common_baseline(&[a.clone(), b]).unwrap().level, 1);
        let c = CpuGroup {
            vendor: "amd".into(),
            level: 3,
        };
        assert!(common_baseline(&[a, c]).is_none());
        assert!(common_baseline(&[]).is_none());
    }
}
