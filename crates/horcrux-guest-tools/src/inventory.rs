//! `horcrux-guest-agent inventory`: list the guest's installed packages as
//! JSON on stdout, so the node can turn it into an SBOM and scan a *running*
//! VM for vulnerabilities without ever touching its live disk.
//!
//! Linux: the first package manager found (dpkg, rpm, apk, Portage) plus
//! `/etc/os-release`. Windows: the "Uninstall" registry keys (installed
//! programs). The output matches `horcrux_common::security::GuestInventory`;
//! the parsers are pure so they can be tested without the real tools.
use serde::Serialize;
use std::process::Command;

#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub struct Pkg {
    pub name: String,
    pub version: String,
    #[serde(rename = "type")]
    pub pkg_type: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub arch: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Inventory {
    pub os: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub distro_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub distro_version: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub distro_like: Vec<String>,
    pub packages: Vec<Pkg>,
}

/// Collect the inventory and print it as JSON. Returns the process exit code.
pub fn run() -> i32 {
    let inv = collect();
    match serde_json::to_string(&inv) {
        Ok(s) => {
            println!("{s}");
            if inv.packages.is_empty() {
                eprintln!("horcrux-guest-agent: no packages found (unsupported package manager?)");
                1
            } else {
                0
            }
        }
        Err(e) => {
            eprintln!("horcrux-guest-agent: inventory: {e}");
            2
        }
    }
}

#[cfg(target_os = "windows")]
fn collect() -> Inventory {
    Inventory {
        os: "windows".into(),
        packages: windows_packages(),
        ..Default::default()
    }
}

#[cfg(not(target_os = "windows"))]
fn collect() -> Inventory {
    let (distro_id, distro_version, distro_like) =
        parse_os_release(&std::fs::read_to_string("/etc/os-release").unwrap_or_default());
    Inventory {
        os: "linux".into(),
        distro_id,
        distro_version,
        distro_like,
        packages: linux_packages(),
    }
}

/// os-release ID, VERSION_ID and ID_LIKE.
pub fn parse_os_release(text: &str) -> (String, String, Vec<String>) {
    let mut id = String::new();
    let mut ver = String::new();
    let mut like = Vec::new();
    for l in text.lines() {
        let l = l.trim();
        let Some((k, v)) = l.split_once('=') else {
            continue;
        };
        let v = v.trim().trim_matches('"').to_string();
        match k.trim() {
            "ID" => id = v,
            "VERSION_ID" => ver = v,
            "ID_LIKE" => like = v.split_whitespace().map(String::from).collect(),
            _ => {}
        }
    }
    (id, ver, like)
}

#[cfg(not(target_os = "windows"))]
fn linux_packages() -> Vec<Pkg> {
    // dpkg (deb), rpm, apk, then Portage; the first that answers wins.
    if which("dpkg-query") {
        if let Some(o) = out(
            "dpkg-query",
            &["-W", "-f=${Package}\t${Version}\t${Architecture}\n"],
        ) {
            let p = parse_dpkg(&o);
            if !p.is_empty() {
                return p;
            }
        }
    }
    if which("rpm") {
        if let Some(o) = out(
            "rpm",
            &["-qa", "--qf", "%{NAME}\t%{VERSION}-%{RELEASE}\t%{ARCH}\n"],
        ) {
            let p = parse_rpm(&o);
            if !p.is_empty() {
                return p;
            }
        }
    }
    if which("apk") {
        if let Some(o) = out("apk", &["info", "-v"]) {
            let p = parse_apk(&o);
            if !p.is_empty() {
                return p;
            }
        }
    }
    if which("qlist") {
        if let Some(o) = out("qlist", &["-ICv"]) {
            let p = parse_portage(&o);
            if !p.is_empty() {
                return p;
            }
        }
    }
    parse_portage_dir("/var/db/pkg")
}

/// `dpkg-query -W -f='${Package}\t${Version}\t${Architecture}\n'`.
pub fn parse_dpkg(text: &str) -> Vec<Pkg> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split('\t');
            let name = it.next()?.trim();
            let version = it.next().unwrap_or("").trim();
            let arch = it.next().unwrap_or("").trim();
            (!name.is_empty() && !version.is_empty()).then(|| Pkg {
                name: name.to_string(),
                version: version.to_string(),
                pkg_type: "deb".into(),
                arch: arch.to_string(),
            })
        })
        .collect()
}

/// `rpm -qa --qf '%{NAME}\t%{VERSION}-%{RELEASE}\t%{ARCH}\n'`.
pub fn parse_rpm(text: &str) -> Vec<Pkg> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split('\t');
            let name = it.next()?.trim();
            let version = it.next().unwrap_or("").trim();
            let arch = it.next().unwrap_or("").trim();
            (!name.is_empty() && !version.is_empty()).then(|| Pkg {
                name: name.to_string(),
                version: version.to_string(),
                pkg_type: "rpm".into(),
                arch: if arch == "(none)" { "" } else { arch }.to_string(),
            })
        })
        .collect()
}

/// `apk info -v` lines are `<name>-<version>-r<rev>`; the name ends at the
/// last `-` that is followed by a digit.
pub fn parse_apk(text: &str) -> Vec<Pkg> {
    text.lines()
        .filter_map(|l| {
            let s = l.trim();
            if s.is_empty() {
                return None;
            }
            let (name, version) = split_name_version(s)?;
            Some(Pkg {
                name,
                version,
                pkg_type: "apk".into(),
                arch: String::new(),
            })
        })
        .collect()
}

/// Split `foo-bar-1.2.3-r0` into (`foo-bar`, `1.2.3-r0`) at the first `-`
/// that starts the version (a `-` followed by a digit).
fn split_name_version(s: &str) -> Option<(String, String)> {
    let bytes = s.as_bytes();
    for (i, w) in bytes.windows(2).enumerate() {
        if w[0] == b'-' && w[1].is_ascii_digit() {
            return Some((s[..i].to_string(), s[i + 1..].to_string()));
        }
    }
    None
}

/// `qlist -ICv` prints `category/name-version` lines; Portage package type.
pub fn parse_portage(text: &str) -> Vec<Pkg> {
    text.lines()
        .filter_map(|l| {
            let s = l.trim();
            let (cn, version) = split_name_version(s)?;
            // cn is `category/name`; keep it as the package name.
            Some(Pkg {
                name: cn,
                version,
                pkg_type: "portage".into(),
                arch: String::new(),
            })
        })
        .collect()
}

/// Portage without `qlist`: walk `/var/db/pkg/<cat>/<pf>` directories.
#[cfg(not(target_os = "windows"))]
fn parse_portage_dir(base: &str) -> Vec<Pkg> {
    let mut out = Vec::new();
    let Ok(cats) = std::fs::read_dir(base) else {
        return out;
    };
    for cat in cats.flatten() {
        let cname = cat.file_name().to_string_lossy().to_string();
        let Ok(pfs) = std::fs::read_dir(cat.path()) else {
            continue;
        };
        for pf in pfs.flatten() {
            let pf = pf.file_name().to_string_lossy().to_string();
            if let Some((name, version)) = split_name_version(&pf) {
                out.push(Pkg {
                    name: format!("{cname}/{name}"),
                    version,
                    pkg_type: "portage".into(),
                    arch: String::new(),
                });
            }
        }
    }
    out
}

#[cfg(target_os = "windows")]
fn windows_packages() -> Vec<Pkg> {
    // PowerShell reads the Uninstall keys (HKLM native + WOW6432Node + HKCU);
    // DisplayName<TAB>DisplayVersion per installed program.
    const PS: &str = "$k='HKLM:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall',\
'HKLM:\\SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall',\
'HKCU:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall';\
Get-ChildItem $k -ErrorAction SilentlyContinue | ForEach-Object { \
$p=Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue; \
if ($p.DisplayName -and -not $p.SystemComponent) { \
('{0}`t{1}' -f $p.DisplayName,$p.DisplayVersion) } }";
    match out(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", PS],
    ) {
        Some(o) => parse_windows(&o),
        None => Vec::new(),
    }
}

/// `DisplayName<TAB>DisplayVersion` lines from the Uninstall keys.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn parse_windows(text: &str) -> Vec<Pkg> {
    let mut out: Vec<Pkg> = text
        .lines()
        .filter_map(|l| {
            let mut it = l.splitn(2, '\t');
            let name = it.next()?.trim();
            let version = it.next().unwrap_or("").trim();
            (!name.is_empty()).then(|| Pkg {
                name: name.to_string(),
                version: version.to_string(),
                pkg_type: "windows".into(),
                arch: String::new(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.version.cmp(&b.version)));
    out.dedup();
    out
}

#[cfg(not(target_os = "windows"))]
fn which(prog: &str) -> bool {
    std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path).any(|d| {
                let p = d.join(prog);
                p.is_file()
            })
        })
        .unwrap_or(false)
}

/// Run a program and return its stdout (UTF-8, lossy) if it starts and exits 0
/// (or prints anything).
fn out(prog: &str, args: &[&str]) -> Option<String> {
    let o = Command::new(prog).args(args).output().ok()?;
    let s = String::from_utf8_lossy(&o.stdout).into_owned();
    (!s.trim().is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_release_parses() {
        let (id, ver, like) =
            parse_os_release("NAME=\"Ubuntu\"\nID=ubuntu\nID_LIKE=debian\nVERSION_ID=\"22.04\"\n");
        assert_eq!((id.as_str(), ver.as_str()), ("ubuntu", "22.04"));
        assert_eq!(like, vec!["debian"]);
    }

    #[test]
    fn dpkg_and_rpm_parse() {
        let d = parse_dpkg("bash\t5.1-6ubuntu1\tamd64\nzlib1g\t1:1.2.11\tamd64\n\n");
        assert_eq!(d.len(), 2);
        assert_eq!(
            (
                d[0].name.as_str(),
                d[0].version.as_str(),
                d[0].pkg_type.as_str(),
                d[0].arch.as_str()
            ),
            ("bash", "5.1-6ubuntu1", "deb", "amd64")
        );
        let r = parse_rpm("openssl\t3.0.7-1.el9\tx86_64\ngpg-pubkey\t1\t(none)\n");
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].pkg_type, "rpm");
        assert_eq!(r[1].arch, "");
    }

    #[test]
    fn apk_and_portage_parse() {
        let a = parse_apk("musl-1.2.4-r2\nbusybox-1.36.1-r15\n");
        assert_eq!(a.len(), 2);
        assert_eq!(
            (
                a[0].name.as_str(),
                a[0].version.as_str(),
                a[0].pkg_type.as_str()
            ),
            ("musl", "1.2.4-r2", "apk")
        );
        let p = parse_portage("app-admin/sudo-1.9.15\nsys-apps/openrc-0.54\n");
        assert_eq!(p.len(), 2);
        assert_eq!(
            (
                p[0].name.as_str(),
                p[0].version.as_str(),
                p[0].pkg_type.as_str()
            ),
            ("app-admin/sudo", "1.9.15", "portage")
        );
    }

    #[test]
    fn windows_uninstall_parses_and_dedups() {
        let w = parse_windows("7-Zip 23.01\t23.01\nMozilla Firefox\t121.0\n7-Zip 23.01\t23.01\n");
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].name, "7-Zip 23.01");
        assert_eq!(w[0].pkg_type, "windows");
        assert_eq!(w[1].name, "Mozilla Firefox");
    }

    #[test]
    fn inventory_serializes_to_the_shared_schema() {
        let inv = Inventory {
            os: "linux".into(),
            distro_id: "alpine".into(),
            distro_version: "3.19".into(),
            distro_like: vec![],
            packages: vec![Pkg {
                name: "musl".into(),
                version: "1.2.4-r2".into(),
                pkg_type: "apk".into(),
                arch: String::new(),
            }],
        };
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&inv).unwrap()).unwrap();
        assert_eq!(v["os"], "linux");
        assert_eq!(v["distro_id"], "alpine");
        assert_eq!(v["packages"][0]["type"], "apk");
        // empty arch and distro_like are omitted, not null
        assert!(v["packages"][0].get("arch").is_none());
        assert!(v.get("distro_like").is_none());
    }
}
