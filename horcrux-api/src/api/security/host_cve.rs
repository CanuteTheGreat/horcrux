//! Host vulnerability exposure: the node's own installed packages matched
//! against the Gentoo security advisories (GLSA) of the synced tree.
//!
//! This reads the same data and applies the same rules as `glsa-check -t all`
//! (gentoolkit / Portage `glsa.py`): an installed package is affected when it
//! matches a `<vulnerable>` range of an advisory and none of its
//! `<unaffected>` ranges (a slot restriction narrows both; the `r*` ranges
//! only compare the revision of one exact version; `arch` limits an entry).
//! It is done in-process, from the files, so it needs no Python and no
//! privileged program, and a fake root can be tested without Portage.
//!
//! Paths are read below `HORCRUX_SYSROOT` (tests use a fake root). Nothing is
//! changed on the host: the card shows the update command, it never runs it.
use super::super::system_admin::rooted;
use horcrux_common::security::{HostAdvisory, HostAffectedPackage, HostCveReport, HostCveSettings};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::Path;

// ------------------------------------------------------------ versions

/// A Portage version (PMS section 3): `1.2.3b_rc1_p2-r3`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    nums: Vec<String>,
    letter: Option<char>,
    /// (rank, number): alpha < beta < pre < rc < (none) < p.
    suffixes: Vec<(i8, u64)>,
    revision: u64,
}

impl Version {
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim();
        let (body, revision) = match s.rfind("-r") {
            Some(i) if s[i + 2..].chars().all(|c| c.is_ascii_digit()) && s.len() > i + 2 => {
                (&s[..i], s[i + 2..].parse().ok()?)
            }
            _ => (s, 0),
        };
        let mut parts = body.split('_');
        let main = parts.next()?;
        let mut letter = None;
        let mut numpart = main;
        if let Some(last) = main.chars().last() {
            if last.is_ascii_lowercase() {
                letter = Some(last);
                numpart = &main[..main.len() - 1];
            }
        }
        let nums: Vec<String> = numpart.split('.').map(|x| x.to_string()).collect();
        if nums.is_empty()
            || nums
                .iter()
                .any(|n| n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()))
        {
            return None;
        }
        let mut suffixes = Vec::new();
        for p in parts {
            let (rank, rest) = if let Some(r) = p.strip_prefix("alpha") {
                (0, r)
            } else if let Some(r) = p.strip_prefix("beta") {
                (1, r)
            } else if let Some(r) = p.strip_prefix("pre") {
                (2, r)
            } else if let Some(r) = p.strip_prefix("rc") {
                (3, r)
            } else {
                (5, p.strip_prefix('p')?)
            };
            let n = if rest.is_empty() {
                0
            } else {
                rest.parse().ok()?
            };
            suffixes.push((rank, n));
        }
        Some(Version {
            nums,
            letter,
            suffixes,
            revision,
        })
    }

    /// Compare ignoring the revision.
    fn cmp_base(&self, o: &Version) -> Ordering {
        for i in 0..self.nums.len().max(o.nums.len()) {
            let (a, b) = match (self.nums.get(i), o.nums.get(i)) {
                (Some(a), Some(b)) => (a, b),
                (Some(_), None) => return Ordering::Greater,
                (None, Some(_)) => return Ordering::Less,
                (None, None) => break,
            };
            let ord = if i > 0 && (a.starts_with('0') || b.starts_with('0')) {
                a.trim_end_matches('0').cmp(b.trim_end_matches('0'))
            } else {
                let (a, b) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
                a.len().cmp(&b.len()).then_with(|| a.cmp(b))
            };
            if ord != Ordering::Equal {
                return ord;
            }
        }
        match self.letter.cmp(&o.letter) {
            Ordering::Equal => {}
            x => return x,
        }
        for i in 0..self.suffixes.len().max(o.suffixes.len()) {
            // A missing suffix ranks between rc and p.
            let a = self.suffixes.get(i).copied().unwrap_or((4, 0));
            let b = o.suffixes.get(i).copied().unwrap_or((4, 0));
            match a.cmp(&b) {
                Ordering::Equal => {}
                x => return x,
            }
        }
        Ordering::Equal
    }

    pub fn cmp_full(&self, o: &Version) -> Ordering {
        self.cmp_base(o)
            .then_with(|| self.revision.cmp(&o.revision))
    }
}

/// Split an installed `name-version[-rN]` directory name.
pub fn split_pf(pf: &str) -> Option<(String, String)> {
    let mut idx = None;
    for (i, c) in pf.char_indices().rev() {
        if c == '-' {
            let rest = &pf[i + 1..];
            if rest.starts_with(|c: char| c.is_ascii_digit()) && Version::parse(rest).is_some() {
                idx = Some(i);
                break;
            }
        }
    }
    let i = idx?;
    Some((pf[..i].to_string(), pf[i + 1..].to_string()))
}

// ------------------------------------------------------------ installed

#[derive(Debug, Clone)]
pub struct Installed {
    pub package: String,
    pub version: String,
    pub slot: String,
    parsed: Version,
}

pub fn read_installed(vdb: &Path) -> Vec<Installed> {
    let mut out = Vec::new();
    let Ok(cats) = std::fs::read_dir(vdb) else {
        return out;
    };
    for cat in cats.flatten() {
        let cname = cat.file_name().to_string_lossy().to_string();
        if !cname.contains('-') && cname != "virtual" || cname.starts_with('.') {
            continue;
        }
        let Ok(pkgs) = std::fs::read_dir(cat.path()) else {
            continue;
        };
        for p in pkgs.flatten() {
            let pf = p.file_name().to_string_lossy().to_string();
            if pf.starts_with("-MERGING-") || pf.starts_with('.') {
                continue;
            }
            let Some((name, ver)) = split_pf(&pf) else {
                continue;
            };
            let Some(parsed) = Version::parse(&ver) else {
                continue;
            };
            let slot = std::fs::read_to_string(p.path().join("SLOT"))
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            out.push(Installed {
                package: format!("{cname}/{name}"),
                version: ver,
                slot,
                parsed,
            });
        }
    }
    out
}

// ------------------------------------------------------------ minimal XML

#[derive(Debug, Default)]
struct Node {
    name: String,
    attrs: BTreeMap<String, String>,
    text: String,
    kids: Vec<Node>,
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn parse_attrs(s: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && (b[i] as char).is_whitespace() {
            i += 1;
        }
        let ks = i;
        while i < b.len() && b[i] != b'=' && !(b[i] as char).is_whitespace() {
            i += 1;
        }
        let key = s[ks..i].to_string();
        while i < b.len() && (b[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] != b'=' {
            break;
        }
        i += 1;
        while i < b.len() && (b[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= b.len() || (b[i] != b'"' && b[i] != b'\'') {
            break;
        }
        let q = b[i];
        i += 1;
        let vs = i;
        while i < b.len() && b[i] != q {
            i += 1;
        }
        m.insert(key, unescape(&s[vs..i.min(b.len())]));
        i += 1;
    }
    m
}

/// Parse the element tree of a small, well-formed XML document.
fn parse_xml(src: &str) -> Option<Node> {
    let mut stack: Vec<Node> = vec![Node::default()];
    let mut rest = src;
    while let Some(lt) = rest.find('<') {
        if let Some(t) = stack.last_mut() {
            t.text.push_str(&unescape(&rest[..lt]));
        }
        rest = &rest[lt..];
        if let Some(r) = rest.strip_prefix("<!--") {
            rest = &r[r.find("-->")? + 3..];
        } else if let Some(r) = rest.strip_prefix("<![CDATA[") {
            let e = r.find("]]>")?;
            stack.last_mut()?.text.push_str(&r[..e]);
            rest = &r[e + 3..];
        } else if rest.starts_with("<?") || rest.starts_with("<!") {
            rest = &rest[rest.find('>')? + 1..];
        } else if let Some(r) = rest.strip_prefix("</") {
            let e = r.find('>')?;
            let done = stack.pop()?;
            if stack.is_empty() || done.name != r[..e].trim() {
                return None;
            }
            stack.last_mut()?.kids.push(done);
            rest = &r[e + 1..];
        } else {
            let e = rest.find('>')?;
            let inner = &rest[1..e];
            let (inner, selfclose) = match inner.strip_suffix('/') {
                Some(i) => (i, true),
                None => (inner, false),
            };
            let (name, attrs) = match inner.find(char::is_whitespace) {
                Some(i) => (&inner[..i], parse_attrs(&inner[i..])),
                None => (inner, BTreeMap::new()),
            };
            let n = Node {
                name: name.to_string(),
                attrs,
                ..Default::default()
            };
            if selfclose {
                stack.last_mut()?.kids.push(n);
            } else {
                stack.push(n);
            }
            rest = &rest[e + 1..];
        }
    }
    if stack.len() != 1 {
        return None;
    }
    stack.pop()?.kids.into_iter().next()
}

impl Node {
    fn child(&self, name: &str) -> Option<&Node> {
        self.kids.iter().find(|k| k.name == name)
    }
    fn children<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> {
        self.kids.iter().filter(move |k| k.name == name)
    }
    fn text(&self) -> String {
        self.text.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

// ------------------------------------------------------------ advisories

/// A version restriction of an advisory (`<vulnerable>` / `<unaffected>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Range {
    pub op: String,
    pub version: String,
    pub slot: String,
}

#[derive(Debug, Clone)]
pub struct AdvPackage {
    pub name: String,
    pub arch: String,
    pub vulnerable: Vec<Range>,
    pub unaffected: Vec<Range>,
}

#[derive(Debug, Clone)]
pub struct Glsa {
    pub id: String,
    pub title: String,
    pub synopsis: String,
    pub impact: String,
    pub cves: Vec<String>,
    pub packages: Vec<AdvPackage>,
}

pub fn parse_glsa(src: &str) -> Option<Glsa> {
    let root = parse_xml(src)?;
    if root.name != "glsa" {
        return None;
    }
    let id = root.attrs.get("id")?.clone();
    let mut packages = Vec::new();
    for aff in root.children("affected") {
        for p in aff.children("package") {
            let ranges = |tag: &str| -> Vec<Range> {
                p.children(tag)
                    .map(|r| Range {
                        op: r.attrs.get("range").cloned().unwrap_or_default(),
                        version: r.text(),
                        slot: r.attrs.get("slot").cloned().unwrap_or_default(),
                    })
                    .collect()
            };
            packages.push(AdvPackage {
                name: p.attrs.get("name").cloned().unwrap_or_default(),
                arch: p.attrs.get("arch").cloned().unwrap_or_else(|| "*".into()),
                vulnerable: ranges("vulnerable"),
                unaffected: ranges("unaffected"),
            });
        }
    }
    let mut cves = Vec::new();
    if let Some(refs) = root.child("references") {
        for u in refs.children("uri") {
            let t = u.text();
            if t.starts_with("CVE-") && !cves.contains(&t) {
                cves.push(t);
            }
        }
    }
    Some(Glsa {
        id,
        title: root.child("title").map(Node::text).unwrap_or_default(),
        synopsis: root.child("synopsis").map(Node::text).unwrap_or_default(),
        impact: root
            .child("impact")
            .and_then(|i| i.attrs.get("type").cloned())
            .unwrap_or_default(),
        cves,
        packages,
    })
}

fn slot_ok(r: &Range, installed: &Installed) -> bool {
    let want = r.slot.trim();
    if want.is_empty() || want == "*" {
        return true;
    }
    let have = installed.slot.split('/').next().unwrap_or("");
    want.split('/').next().unwrap_or("") == have
}

/// Does `installed` match one range (Portage `glsa.py` semantics)?
pub fn range_matches(r: &Range, installed: &Installed) -> bool {
    if !slot_ok(r, installed) {
        return false;
    }
    let Some(v) = Version::parse(&r.version) else {
        return false;
    };
    let iv = &installed.parsed;
    match r.op.as_str() {
        "lt" => iv.cmp_full(&v) == Ordering::Less,
        "le" => iv.cmp_full(&v) != Ordering::Greater,
        "eq" => iv.cmp_full(&v) == Ordering::Equal,
        "ge" => iv.cmp_full(&v) != Ordering::Less,
        "gt" => iv.cmp_full(&v) == Ordering::Greater,
        "ne" => iv.cmp_full(&v) != Ordering::Equal,
        // Revision ranges: same version, only the revision is compared.
        "rlt" | "rle" | "rge" | "rgt" => {
            if iv.cmp_base(&v) != Ordering::Equal {
                return false;
            }
            let o = iv.revision.cmp(&v.revision);
            match r.op.as_str() {
                "rlt" => o == Ordering::Less,
                "rle" => o != Ordering::Greater,
                "rge" => o != Ordering::Less,
                _ => o == Ordering::Greater,
            }
        }
        _ => false,
    }
}

fn describe(r: &Range) -> String {
    let (op, note) = match r.op.as_str() {
        "lt" => ("<", ""),
        "le" => ("<=", ""),
        "eq" => ("=", ""),
        "ge" => (">=", ""),
        "gt" => (">", ""),
        "ne" => ("!=", ""),
        "rlt" => ("<", " (this version only, revision)"),
        "rle" => ("<=", " (this version only, revision)"),
        "rge" => (">=", " (this version only, revision)"),
        "rgt" => (">", " (this version only, revision)"),
        _ => ("", ""),
    };
    let slot = if r.slot.is_empty() || r.slot == "*" {
        String::new()
    } else {
        format!(" (slot {})", r.slot)
    };
    format!("{op}{}{note}{slot}", r.version)
}

/// Gentoo architecture name of this node.
pub fn detect_arch() -> String {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "riscv64" => "riscv",
        "x86" => "x86",
        "arm" => "arm",
        "powerpc64" => "ppc64",
        o => o,
    }
    .to_string()
}

fn arch_ok(entry: &str, arch: &str) -> bool {
    let e = entry.trim();
    e.is_empty() || e == "*" || e.split_whitespace().any(|a| a == arch)
}

fn severity(impact: &str) -> &'static str {
    match impact {
        "high" => "high",
        "normal" => "medium",
        "low" => "low",
        _ => "medium",
    }
}

fn sev_rank(s: &str) -> u8 {
    match s {
        "critical" => 4,
        "high" => 3,
        "medium" => 2,
        _ => 1,
    }
}

/// Advisories that affect any of `installed`.
pub fn match_advisories(
    glsas: &[Glsa],
    installed: &[Installed],
    arch: &str,
    ignore: &[String],
) -> (Vec<HostAdvisory>, u32) {
    let mut by_pkg: BTreeMap<&str, Vec<&Installed>> = BTreeMap::new();
    for i in installed {
        by_pkg.entry(i.package.as_str()).or_default().push(i);
    }
    let mut out = Vec::new();
    let mut ignored = 0;
    for g in glsas {
        let mut pkgs: Vec<HostAffectedPackage> = Vec::new();
        for p in &g.packages {
            if !arch_ok(&p.arch, arch) {
                continue;
            }
            let Some(list) = by_pkg.get(p.name.as_str()) else {
                continue;
            };
            for i in list {
                let vuln = p.vulnerable.iter().any(|r| range_matches(r, i));
                let fixed = p.unaffected.iter().any(|r| range_matches(r, i));
                if vuln && !fixed {
                    let mut fx: Vec<String> = p.unaffected.iter().map(describe).collect();
                    fx.dedup();
                    if !pkgs
                        .iter()
                        .any(|x| x.package == i.package && x.installed == i.version)
                    {
                        pkgs.push(HostAffectedPackage {
                            package: i.package.clone(),
                            installed: i.version.clone(),
                            slot: i.slot.clone(),
                            fixed: fx,
                        });
                    }
                }
            }
        }
        if pkgs.is_empty() {
            continue;
        }
        if ignore.iter().any(|x| x == &g.id) {
            ignored += 1;
            continue;
        }
        out.push(HostAdvisory {
            id: g.id.clone(),
            title: g.title.clone(),
            severity: severity(&g.impact).into(),
            synopsis: g.synopsis.clone(),
            cves: g.cves.clone(),
            url: format!("https://security.gentoo.org/glsa/{}", g.id),
            packages: pkgs,
        });
    }
    out.sort_by(|a, b| {
        sev_rank(&b.severity)
            .cmp(&sev_rank(&a.severity))
            .then_with(|| b.id.cmp(&a.id))
    });
    (out, ignored)
}

/// Read every GLSA of the tree.
pub fn read_glsas(dir: &Path) -> Vec<Glsa> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if !(n.starts_with("glsa-") && n.ends_with(".xml")) {
            continue;
        }
        if let Some(g) = std::fs::read_to_string(e.path())
            .ok()
            .and_then(|s| parse_glsa(&s))
        {
            out.push(g);
        }
    }
    out
}

/// Evaluate the host now (blocking: reads a few thousand small files).
pub fn evaluate(s: &HostCveSettings, node: &str) -> HostCveReport {
    let arch = if s.arch.trim().is_empty() {
        detect_arch()
    } else {
        s.arch.trim().to_string()
    };
    let mut r = HostCveReport {
        node: node.to_string(),
        checked_at: chrono::Utc::now().timestamp(),
        arch: arch.clone(),
        update_command: s.update_command.clone(),
        ..Default::default()
    };
    let vdb = rooted(&s.vdb_dir);
    let gdir = rooted(&s.glsa_dir);
    if !vdb.is_dir() {
        r.reason = format!(
            "This node has no Portage package database ({}); the host check covers Gentoo-based nodes only.",
            s.vdb_dir
        );
        return r;
    }
    let glsas = read_glsas(&gdir);
    if glsas.is_empty() {
        r.reason = format!(
            "No Gentoo security advisories found in {}. Sync the Gentoo tree (emerge --sync) or set host_cve.glsa_dir.",
            s.glsa_dir
        );
        return r;
    }
    let installed = read_installed(&vdb);
    r.supported = true;
    r.installed_packages = installed.len() as u32;
    r.advisories_checked = glsas.len() as u32;
    r.newest_advisory = glsas.iter().map(|g| g.id.clone()).max().unwrap_or_default();
    let (adv, ignored) = match_advisories(&glsas, &installed, &arch, &s.ignore);
    let mut pk: Vec<(&str, &str)> = adv
        .iter()
        .flat_map(|a| {
            a.packages
                .iter()
                .map(|p| (p.package.as_str(), p.installed.as_str()))
        })
        .collect();
    pk.sort();
    pk.dedup();
    r.affected_packages = pk.len() as u32;
    r.ignored = ignored;
    r.advisories = adv;
    r
}

// ------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;

    fn inst(pkg: &str, ver: &str, slot: &str) -> Installed {
        Installed {
            package: pkg.into(),
            version: ver.into(),
            slot: slot.into(),
            parsed: Version::parse(ver).unwrap(),
        }
    }

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn versions_follow_portage_order() {
        let order = [
            "1.0_alpha1",
            "1.0_beta2",
            "1.0_pre1",
            "1.0_rc1",
            "1.0",
            "1.0-r1",
            "1.0_p1",
            "1.0a",
            "1.0.1",
            "1.1",
            "1.10",
            "2",
        ];
        for w in order.windows(2) {
            assert_eq!(
                v(w[0]).cmp_full(&v(w[1])),
                Ordering::Less,
                "{} < {}",
                w[0],
                w[1]
            );
        }
        assert_eq!(v("1.2").cmp_full(&v("1.2-r0")), Ordering::Equal);
        assert_eq!(v("1.02").cmp_base(&v("1.2")), Ordering::Less);
        assert!(Version::parse("x1").is_none());
    }

    #[test]
    fn installed_names_split() {
        assert_eq!(
            split_pf("openssl-3.0.13-r2"),
            Some(("openssl".into(), "3.0.13-r2".into()))
        );
        assert_eq!(
            split_pf("gtk+-3.24.41"),
            Some(("gtk+".into(), "3.24.41".into()))
        );
        assert_eq!(
            split_pf("perl-5.38.2_p1"),
            Some(("perl".into(), "5.38.2_p1".into()))
        );
        assert_eq!(
            split_pf("xorg-server-21.1.9"),
            Some(("xorg-server".into(), "21.1.9".into()))
        );
        assert_eq!(split_pf("noversion"), None);
    }

    const GLSA: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE glsa SYSTEM "http://www.gentoo.org/dtd/glsa.dtd">
<glsa id="202611-03">
  <title>OpenSSL: Multiple vulnerabilities</title>
  <synopsis>Several issues &amp; one more were found in OpenSSL.</synopsis>
  <product type="ebuild">openssl</product>
  <announced>2026-11-01</announced>
  <bug>900001</bug>
  <access>remote</access>
  <affected>
    <package name="dev-libs/openssl" auto="yes" arch="*">
      <unaffected range="ge">3.0.14</unaffected>
      <unaffected range="rge" slot="1.1">1.1.1w-r1</unaffected>
      <vulnerable range="lt">3.0.14</vulnerable>
    </package>
    <package name="app-arch/foo" auto="yes" arch="sparc">
      <vulnerable range="lt">9</vulnerable>
    </package>
  </affected>
  <impact type="high"><p>x</p></impact>
  <references>
    <uri link="https://nvd.nist.gov/vuln/detail/CVE-2026-1111">CVE-2026-1111</uri>
    <uri link="https://nvd.nist.gov/vuln/detail/CVE-2026-1112">CVE-2026-1112</uri>
  </references>
</glsa>"#;

    #[test]
    fn glsa_is_parsed() {
        let g = parse_glsa(GLSA).unwrap();
        assert_eq!(g.id, "202611-03");
        assert_eq!(
            g.synopsis,
            "Several issues & one more were found in OpenSSL."
        );
        assert_eq!(g.impact, "high");
        assert_eq!(g.cves, vec!["CVE-2026-1111", "CVE-2026-1112"]);
        assert_eq!(g.packages.len(), 2);
        assert_eq!(g.packages[0].unaffected[1].slot, "1.1");
        assert!(parse_glsa("<glsa id=\"x\"><title></glsa>").is_none());
    }

    #[test]
    fn vulnerable_unaffected_slot_and_arch() {
        let g = vec![parse_glsa(GLSA).unwrap()];
        let hit = |ver: &str, slot: &str, arch: &str| {
            match_advisories(&g, &[inst("dev-libs/openssl", ver, slot)], arch, &[])
                .0
                .len()
        };
        assert_eq!(hit("3.0.13", "0/3", "amd64"), 1);
        assert_eq!(hit("3.0.14", "0/3", "amd64"), 0);
        assert_eq!(hit("3.0.13-r3", "0/3", "amd64"), 1);
        // slot 1.1: 1.1.1w-r1 is unaffected, 1.1.1w (r0) is below the fix but
        // not in the vulnerable range (lt 3.0.14 covers it, rge only fixes r1+)
        assert_eq!(hit("1.1.1w-r1", "1.1", "amd64"), 0);
        // the arch="sparc" entry never applies to amd64
        let r = match_advisories(&g, &[inst("app-arch/foo", "1", "0")], "amd64", &[]);
        assert!(r.0.is_empty());
        let r = match_advisories(&g, &[inst("app-arch/foo", "1", "0")], "sparc", &[]);
        assert_eq!(r.0.len(), 1);
    }

    #[test]
    fn revision_ranges_compare_only_the_revision() {
        let r = Range {
            op: "rge".into(),
            version: "2.4-r2".into(),
            slot: String::new(),
        };
        assert!(range_matches(&r, &inst("a/b", "2.4-r2", "0")));
        assert!(range_matches(&r, &inst("a/b", "2.4-r5", "0")));
        assert!(!range_matches(&r, &inst("a/b", "2.4-r1", "0")));
        // a different version is never in a revision range
        assert!(!range_matches(&r, &inst("a/b", "2.5-r9", "0")));
    }

    #[test]
    fn report_fields_and_ignore() {
        let g = vec![parse_glsa(GLSA).unwrap()];
        let i = [inst("dev-libs/openssl", "3.0.13", "0/3")];
        let (a, ign) = match_advisories(&g, &i, "amd64", &[]);
        assert_eq!(ign, 0);
        assert_eq!(a[0].severity, "high");
        assert_eq!(a[0].url, "https://security.gentoo.org/glsa/202611-03");
        assert_eq!(a[0].packages[0].fixed[0], ">=3.0.14");
        assert!(a[0].packages[0].fixed[1].contains("slot 1.1"));
        let (a, ign) = match_advisories(&g, &i, "amd64", &["202611-03".into()]);
        assert!(a.is_empty());
        assert_eq!(ign, 1);
    }

    fn fake_root(root: &Path) {
        let g = root.join("var/db/repos/gentoo/metadata/glsa");
        std::fs::create_dir_all(&g).unwrap();
        std::fs::write(g.join("glsa-202611-03.xml"), GLSA).unwrap();
        std::fs::write(g.join("glsa-broken.xml"), "<glsa").unwrap();
        for (cat, pf, slot) in [
            ("dev-libs", "openssl-3.0.13-r1", "0/3"),
            ("sys-apps", "coreutils-9.5", "0"),
        ] {
            let d = root.join("var/db/pkg").join(cat).join(pf);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("SLOT"), format!("{slot}\n")).unwrap();
        }
        std::fs::create_dir_all(root.join("var/db/pkg/dev-libs/-MERGING-foo-1")).unwrap();
    }

    #[test]
    fn fake_sysroot_end_to_end() {
        let _g = crate::api::system_admin::SYSROOT_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let root = tempfile::tempdir().unwrap();
        fake_root(root.path());
        std::env::set_var("HORCRUX_SYSROOT", root.path());
        let s = HostCveSettings {
            arch: "amd64".into(),
            ..Default::default()
        };
        let r = evaluate(&s, "n1");
        assert!(r.supported, "{}", r.reason);
        assert_eq!(r.installed_packages, 2);
        assert_eq!(r.advisories_checked, 1);
        assert_eq!(r.advisories.len(), 1);
        assert_eq!(r.affected_packages, 1);
        assert_eq!(r.newest_advisory, "202611-03");
        // no package database -> not supported, with the reason
        std::fs::remove_dir_all(root.path().join("var/db/pkg")).unwrap();
        let r = evaluate(&s, "n1");
        assert!(!r.supported);
        assert!(r.reason.contains("Portage"));
        std::env::remove_var("HORCRUX_SYSROOT");
    }

    /// Real-data run: `HX_REAL_ROOT=<stage3 root> cargo test ... real_root -- --ignored --nocapture`
    /// prints one advisory id per line (compared with `glsa-check -t all`).
    #[test]
    #[ignore]
    fn real_root_prints_affecting_advisories() {
        let root = std::env::var("HX_REAL_ROOT").expect("HX_REAL_ROOT");
        std::env::set_var("HORCRUX_SYSROOT", root);
        let s = HostCveSettings::default();
        let r = evaluate(&s, "real");
        assert!(r.supported, "{}", r.reason);
        println!(
            "REAL installed={} checked={} affecting={}",
            r.installed_packages,
            r.advisories_checked,
            r.advisories.len()
        );
        for a in &r.advisories {
            println!("ADV {}", a.id);
        }
    }
}
