//! Installing Ceph and choosing its release (Proxmox "Ceph > Install").
//!
//! * `GET  /api/ceph/releases`       the releases the package repositories carry for this
//!   architecture (by major version: Squid 19, Tentacle 20, Umbrella 21, ...), the installed
//!   one, the pinned one and the recommended one
//! * `POST /api/ceph/install`        `{release, bootstrap}`: pin the release, install it
//!   through the package manager and, with `bootstrap.mode` = `new`, create a cluster
//!   (first monitor + manager on this node) or, with `join`, add this node's monitor and
//!   manager to an existing cluster (`peer`, an administrator login there). A task.
//! * `POST /api/ceph/join-material`  the member's side of a join: ceph.conf and the
//!   admin keyring (administrators only)
//! * `GET  /api/ceph/upgrade`        daemon versions across the cluster and the next step
//! * `POST /api/ceph/upgrade`        `{release, steps}`: Ceph's upgrade order. `packages`
//!   (pin + update), then on every node `restart-mon`, then `restart-mgr`, `restart-osd`,
//!   `restart-mds`, `restart-rgw`, and once every daemon runs the new release `finish`
//!   (`require-osd-release`, `noout` cleared). A task.
//!
//! The release is carried out as two Portage files Horcrux owns
//! (`package.mask/horcrux-ceph-release` keeps `sys-cluster/ceph` inside the chosen major
//! version, `package.accept_keywords/horcrux-ceph-release` accepts its testing keyword
//! when the release is not stable yet), so later world updates stay on that release.
//! A release the repositories do not carry for this architecture answers 424.
use crate::api::system_admin::portage_tree::{self as pt, Keyword};
use crate::api::system_admin::{data_dir, rooted, sysroot, user_of, which};
use crate::api::tasks;
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::State,
    routing::{get, post},
    Extension, Json, Router,
};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/ceph/releases", get(releases))
        .route("/api/ceph/install", post(install))
        .route("/api/ceph/join-material", post(join_material))
        .route("/api/ceph/upgrade", get(upgrade_status).post(upgrade))
}

type AuthExt = Option<Extension<crate::middleware::auth::AuthUser>>;
type Res<T> = Result<Json<T>, ApiError>;

pub const CEPH_ATOM: &str = "sys-cluster/ceph";
/// File name of both Ceph release pins.
pub const PIN_NAME: &str = "horcrux-ceph-release";
/// Ceph supports upgrading from at most this many major releases back.
pub const MAX_UPGRADE_SPAN: u64 = 2;

/// Upstream's release names (the X of version X.Y.Z; docs.ceph.com/en/latest/releases).
const RELEASE_NAMES: &[(u64, &str)] = &[
    (15, "Octopus"),
    (16, "Pacific"),
    (17, "Quincy"),
    (18, "Reef"),
    (19, "Squid"),
    (20, "Tentacle"),
    (21, "Umbrella"),
];

pub fn release_name(major: u64) -> Option<&'static str> {
    RELEASE_NAMES
        .iter()
        .find(|(m, _)| *m == major)
        .map(|(_, n)| *n)
}

/// `Umbrella (21)`, or `Ceph 22` for a release newer than this build knows by name.
pub fn release_label(major: u64) -> String {
    match release_name(major) {
        Some(n) => format!("{n} ({major})"),
        None => format!("Ceph {major}"),
    }
}

/// `21`, `21.2`, `21.2.0`, `umbrella`, `Umbrella` -> 21.
pub fn parse_release(s: &str) -> Result<u64, String> {
    let t = s.trim();
    if let Some((m, _)) = RELEASE_NAMES
        .iter()
        .find(|(_, n)| n.eq_ignore_ascii_case(t))
    {
        return Ok(*m);
    }
    let major = t
        .trim_start_matches(['v', 'V'])
        .split('.')
        .next()
        .and_then(|m| m.parse::<u64>().ok())
        .filter(|m| *m > 0);
    major.ok_or_else(|| {
        format!(
            "'{s}' is not a Ceph release: give a major version (21) or a release name ({})",
            RELEASE_NAMES
                .iter()
                .map(|(_, n)| n.to_ascii_lowercase())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CephRelease {
    pub major: u64,
    pub name: String,
    pub newest: String,
    pub versions: Vec<String>,
    /// keyword of the newest version on this architecture
    pub keyword: Keyword,
    pub repo: String,
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CephReleases {
    pub arch: String,
    pub package_manager: bool,
    pub installed: Option<String>,
    pub installed_major: Option<u64>,
    pub pinned: Option<u64>,
    pub releases: Vec<CephRelease>,
    /// the newest release available for this architecture
    pub recommended: Option<u64>,
    /// a cluster config exists on this node (`/etc/ceph/ceph.conf`)
    pub configured: bool,
}

pub fn catalog(root: &Path, arch: &str) -> CephReleases {
    let mut by_major: Vec<CephRelease> = Vec::new();
    for e in pt::ebuilds(root, CEPH_ATOM) {
        let kw = pt::keyword_for(&e.keywords, arch);
        if kw == Keyword::Missing {
            continue;
        }
        let major = pt::major_minor(&e.version).0;
        match by_major.iter_mut().find(|r| r.major == major) {
            Some(r) => {
                if !r.versions.contains(&e.version) {
                    r.versions.push(e.version.clone());
                }
            }
            None => by_major.push(CephRelease {
                major,
                name: release_name(major).unwrap_or("").to_string(),
                newest: e.version.clone(),
                versions: vec![e.version.clone()],
                keyword: kw,
                repo: e.repo.clone(),
                available: true,
            }),
        }
    }
    by_major.sort_by(|a, b| b.major.cmp(&a.major));
    let installed = pt::installed(root, CEPH_ATOM).into_iter().next();
    let pinned = pt::read_pin(root, "package.mask", PIN_NAME).and_then(|t| {
        t.lines()
            .find_map(|l| l.strip_prefix("# release: "))
            .and_then(|m| m.trim().parse().ok())
    });
    CephReleases {
        arch: arch.into(),
        package_manager: root.join("var/db/pkg").is_dir(),
        installed_major: installed.as_deref().map(|v| pt::major_minor(v).0),
        installed,
        pinned,
        recommended: by_major.first().map(|r| r.major),
        releases: by_major,
        configured: root.join("etc/ceph/ceph.conf").is_file(),
    }
}

/// The pin files for a release: (accept_keywords, mask).
pub fn pins(rel: &CephRelease, arch: &str) -> (Option<String>, String) {
    let head = format!(
        "# Written by Horcrux: Ceph release {} (Storage > Ceph). Edit the choice there, not this file.\n",
        release_label(rel.major)
    );
    let mask = format!(
        "{head}# release: {}\n<{CEPH_ATOM}-{}\n>={CEPH_ATOM}-{}\n",
        rel.major,
        rel.major,
        rel.major + 1
    );
    let kw = (rel.keyword == Keyword::Testing)
        .then(|| format!("{head}={CEPH_ATOM}-{}* ~{arch}\n", rel.major));
    (kw, mask)
}

fn write_pins(root: &Path, rel: &CephRelease, arch: &str) -> Result<(), ApiError> {
    let (kw, mask) = pins(rel, arch);
    pt::write_pin(root, "package.accept_keywords", PIN_NAME, kw.as_deref())
        .and_then(|_| pt::write_pin(root, "package.mask", PIN_NAME, Some(&mask)))
        .map_err(ApiError::Internal)
}

fn unavailable(major: u64, cat: &CephReleases) -> ApiError {
    let have = cat
        .releases
        .iter()
        .map(|r| release_label(r.major))
        .collect::<Vec<_>>()
        .join(", ");
    ApiError::NotAvailable {
        reason: "ceph-release-unavailable",
        message: format!(
            "Ceph {} is not available for {} in this node's package repositories.",
            release_label(major),
            cat.arch
        ),
        hint: if have.is_empty() {
            "Sync the package repositories (System > Packages > Update) and try again.".into()
        } else {
            format!("Available here: {have}. Sync the repositories if a newer release was just published.")
        },
    }
}

// ------------------------------------------------------------- bootstrap

/// Monitor ports: Ceph's registered msgr2 and legacy msgr1 ports (defaults).
fn default_v2_port() -> u16 {
    3300
}
fn default_v1_port() -> u16 {
    6789
}
fn default_size() -> u8 {
    3
}
fn default_min_size() -> u8 {
    2
}
fn default_wait() -> u64 {
    180
}
fn yes() -> bool {
    true
}

/// What happens after the packages are in.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum Bootstrap {
    /// packages only (a client node, or a cluster set up later)
    #[default]
    None,
    /// create a cluster: the first monitor and manager run on this node
    New(NewCluster),
    /// add this node's monitor and manager to the cluster `peer` belongs to
    Join(JoinCluster),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewCluster {
    /// the network monitors and clients use (CIDR), e.g. `10.0.0.0/24`
    pub public_network: String,
    /// separate replication network (CIDR); empty = the public network
    #[serde(default)]
    pub cluster_network: Option<String>,
    /// this node's monitor address; default: its address in `public_network`
    #[serde(default)]
    pub mon_ip: Option<String>,
    #[serde(default = "default_size")]
    pub pool_size: u8,
    #[serde(default = "default_min_size")]
    pub pool_min_size: u8,
    #[serde(default)]
    pub ports: MonPorts,
    /// seconds to wait for each daemon to answer
    #[serde(default = "default_wait")]
    pub wait_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonPorts {
    #[serde(default = "default_v2_port")]
    pub v2: u16,
    #[serde(default = "default_v1_port")]
    pub v1: u16,
}

impl Default for MonPorts {
    fn default() -> Self {
        MonPorts {
            v2: default_v2_port(),
            v1: default_v1_port(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JoinCluster {
    /// a member's Horcrux API: `host` or `host:port`
    pub peer: String,
    pub username: String,
    pub password: String,
    /// this node's monitor address; default: its address in the cluster's public network
    #[serde(default)]
    pub mon_ip: Option<String>,
    #[serde(default = "yes")]
    pub mon: bool,
    #[serde(default = "yes")]
    pub mgr: bool,
    #[serde(default)]
    pub ports: MonPorts,
    #[serde(default = "default_wait")]
    pub wait_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InstallReq {
    pub release: String,
    #[serde(default)]
    pub bootstrap: Bootstrap,
}

/// `10.0.0.0/24` / `fd00::/64` -> (network, prefix).
pub fn parse_cidr(s: &str) -> Result<(IpAddr, u8), String> {
    let (a, p) = s
        .trim()
        .split_once('/')
        .ok_or_else(|| format!("'{s}' is not a network in CIDR form (10.0.0.0/24)"))?;
    let ip: IpAddr = a.parse().map_err(|_| format!("'{a}' is not an IP address"))?;
    let p: u8 = p.parse().map_err(|_| format!("'{p}' is not a prefix length"))?;
    if p > if ip.is_ipv4() { 32 } else { 128 } {
        return Err(format!("prefix /{p} is too long"));
    }
    Ok((ip, p))
}

pub fn in_network(ip: IpAddr, net: (IpAddr, u8)) -> bool {
    match (ip, net.0) {
        (IpAddr::V4(a), IpAddr::V4(n)) => {
            let m = if net.1 == 0 { 0 } else { u32::MAX << (32 - net.1) };
            u32::from(a) & m == u32::from(n) & m
        }
        (IpAddr::V6(a), IpAddr::V6(n)) => {
            let m = if net.1 == 0 { 0 } else { u128::MAX << (128 - net.1) };
            u128::from(a) & m == u128::from(n) & m
        }
        _ => false,
    }
}

/// This node's first address inside `net`.
pub fn pick_ip(addrs: &[IpAddr], net: (IpAddr, u8)) -> Option<IpAddr> {
    addrs.iter().copied().find(|a| in_network(*a, net))
}

fn local_addrs() -> Vec<IpAddr> {
    let mut v = Vec::new();
    if let Ok(it) = nix::ifaddrs::getifaddrs() {
        for i in it {
            if let Some(a) = i.address {
                if let Some(s) = a.as_sockaddr_in() {
                    v.push(IpAddr::V4(s.ip()));
                } else if let Some(s) = a.as_sockaddr_in6() {
                    v.push(IpAddr::V6(s.ip()));
                }
            }
        }
    }
    v
}

fn short_hostname() -> String {
    hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default()
        .split('.')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn valid_mon_id(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 63
        && !h.starts_with('-')
        && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Single-quote a value for sh.
pub fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn mon_addrv(ip: IpAddr, p: &MonPorts) -> String {
    let h = match ip {
        IpAddr::V4(a) => a.to_string(),
        IpAddr::V6(a) => format!("[{a}]"),
    };
    format!("[v2:{h}:{},v1:{h}:{}]", p.v2, p.v1)
}

/// Everything the install script needs, already validated.
#[derive(Debug, Clone, PartialEq)]
pub struct InstallPlan {
    pub major: u64,
    pub emerge: Vec<String>,
    pub host: String,
    pub step: PlanStep,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlanStep {
    None,
    New {
        fsid: String,
        conf: String,
        mon_ip: IpAddr,
        ports: MonPorts,
        wait: u64,
    },
    Join {
        peer: String,
        mon_ip: IpAddr,
        mon: bool,
        mgr: bool,
        wait: u64,
    },
}

pub fn ceph_conf(fsid: &str, host: &str, mon_ip: IpAddr, n: &NewCluster) -> String {
    let mut c = format!(
        "# Written by Horcrux when this cluster was created.\n[global]\n\
         fsid = {fsid}\nmon_initial_members = {host}\nmon_host = {}\n\
         public_network = {}\n",
        mon_addrv(mon_ip, &n.ports),
        n.public_network.trim()
    );
    if let Some(cn) = n.cluster_network.as_deref().filter(|s| !s.trim().is_empty()) {
        c.push_str(&format!("cluster_network = {}\n", cn.trim()));
    }
    c.push_str(&format!(
        "auth_cluster_required = cephx\nauth_service_required = cephx\nauth_client_required = cephx\n\
         osd_pool_default_size = {}\nosd_pool_default_min_size = {}\n",
        n.pool_size, n.pool_min_size
    ));
    c
}

const SCRIPT_HEAD: &str = r#"#!/bin/sh
# Generated by Horcrux; runs as a task (its output is the task log).
set -eu
say() { printf '== %s\n' "$*"; }
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
# start a Ceph daemon now and at boot, on systemd or OpenRC (Gentoo's ceph init script serves ceph-<type>.<id> links)
svc_start() {
    if [ -d /run/systemd/system ]; then
        systemctl enable "ceph-$1@$2.service"
        systemctl restart "ceph-$1@$2.service"
    else
        [ -e "/etc/init.d/ceph-$1.$2" ] || ln -s ceph "/etc/init.d/ceph-$1.$2"
        rc-update add "ceph-$1.$2" default
        rc-service "ceph-$1.$2" restart
    fi
}
# every local daemon of one type (ids from /var/lib/ceph/<type>/ceph-<id>)
svc_restart_all() {
    for d in /var/lib/ceph/"$1"/ceph-*; do
        [ -d "$d" ] || continue
        id=${d##*/ceph-}
        say "restarting $1.$id"
        if [ -d /run/systemd/system ]; then systemctl restart "ceph-$1@$id.service"; else rc-service "ceph-$1.$id" restart; fi
    done
}
wait_for() {
    n=$1; shift; i=0
    until "$@" >/dev/null 2>&1; do
        i=$((i + 2))
        if [ "$i" -ge "$n" ]; then echo "timed out after ${n}s waiting for: $*" >&2; return 1; fi
        sleep 2
    done
}
in_quorum() { ceph --connect-timeout 5 quorum_status -f json | grep -q "\"$1\""; }
start_mgr() {
    install -d -o ceph -g ceph -m 0750 "/var/lib/ceph/mgr/ceph-$HOST"
    ceph auth get-or-create "mgr.$HOST" mon 'allow profile mgr' osd 'allow *' mds 'allow *' > "/var/lib/ceph/mgr/ceph-$HOST/keyring"
    chown -R ceph:ceph "/var/lib/ceph/mgr/ceph-$HOST"
    svc_start mgr "$HOST"
}
"#;

pub fn install_script(p: &InstallPlan) -> String {
    let mut s = String::from(SCRIPT_HEAD);
    s.push_str(&format!("HOST={}\n", sq(&p.host)));
    s.push_str(&format!(
        "say {}\nemerge {}\nceph --version\n",
        sq(&format!(
            "installing {CEPH_ATOM} (release {})",
            release_label(p.major)
        )),
        p.emerge.iter().map(|a| sq(a)).collect::<Vec<_>>().join(" ")
    ));
    match &p.step {
        PlanStep::None => {
            s.push_str("say 'packages installed; no cluster was created or joined'\n");
        }
        PlanStep::New {
            fsid,
            conf,
            mon_ip,
            ports,
            wait,
        } => {
            s.push_str(&format!(
                "say {}\n\
                 install -d -m 0755 /etc/ceph\n\
                 cat > /etc/ceph/ceph.conf <<'HXEOF'\n{conf}HXEOF\n\
                 chmod 0644 /etc/ceph/ceph.conf\n\
                 ceph-authtool --create-keyring \"$TMP/mon.keyring\" --gen-key -n mon. --cap mon 'allow *'\n\
                 ( umask 077; ceph-authtool --create-keyring /etc/ceph/ceph.client.admin.keyring --gen-key -n client.admin \
                 --cap mon 'allow *' --cap osd 'allow *' --cap mds 'allow *' --cap mgr 'allow *' )\n\
                 install -d -o ceph -g ceph -m 0750 /var/lib/ceph/bootstrap-osd\n\
                 ceph-authtool --create-keyring /var/lib/ceph/bootstrap-osd/ceph.keyring --gen-key -n client.bootstrap-osd \
                 --cap mon 'profile bootstrap-osd' --cap mgr 'allow r'\n\
                 ceph-authtool \"$TMP/mon.keyring\" --import-keyring /etc/ceph/ceph.client.admin.keyring\n\
                 ceph-authtool \"$TMP/mon.keyring\" --import-keyring /var/lib/ceph/bootstrap-osd/ceph.keyring\n\
                 monmaptool --create --addv \"$HOST\" {} --fsid {} \"$TMP/monmap\"\n\
                 chown -R ceph:ceph \"$TMP\" /var/lib/ceph/bootstrap-osd\n\
                 install -d -o ceph -g ceph -m 0750 \"/var/lib/ceph/mon/ceph-$HOST\"\n\
                 ceph-mon --mkfs -i \"$HOST\" --monmap \"$TMP/monmap\" --keyring \"$TMP/mon.keyring\" --setuser ceph --setgroup ceph\n\
                 svc_start mon \"$HOST\"\n\
                 wait_for {wait} in_quorum \"$HOST\"\n\
                 ceph config set mon auth_allow_insecure_global_id_reclaim false\n\
                 say 'starting the manager'\n\
                 start_mgr\n\
                 wait_for {wait} sh -c 'ceph --connect-timeout 5 mgr stat -f json | grep -q \"\\\"available\\\": *true\"'\n\
                 say 'cluster created'\n\
                 ceph -s\n",
                sq(&format!("creating cluster {fsid}: monitor {} on {mon_ip}", p.host)),
                sq(&mon_addrv(*mon_ip, ports)),
                sq(fsid),
            ));
        }
        PlanStep::Join {
            peer,
            mon_ip,
            mon,
            mgr,
            wait,
        } => {
            s.push_str(&format!(
                "say {}\n\
                 chmod 0644 /etc/ceph/ceph.conf\n\
                 chmod 0600 /etc/ceph/ceph.client.admin.keyring\n\
                 wait_for {wait} ceph --connect-timeout 5 -s\n\
                 install -d -o ceph -g ceph -m 0750 /var/lib/ceph/bootstrap-osd\n\
                 ceph auth get client.bootstrap-osd -o /var/lib/ceph/bootstrap-osd/ceph.keyring\n\
                 chown -R ceph:ceph /var/lib/ceph/bootstrap-osd\n",
                sq(&format!("joining the Ceph cluster of {peer}"))
            ));
            if *mon {
                s.push_str(&format!(
                    "say {}\n\
                     grep -q \"^\\[mon\\.$HOST\\]\" /etc/ceph/ceph.conf || printf '\\n[mon.%s]\\npublic_addr = %s\\n' \"$HOST\" {} >> /etc/ceph/ceph.conf\n\
                     ceph auth get mon. -o \"$TMP/mon.keyring\"\n\
                     ceph mon getmap -o \"$TMP/monmap\"\n\
                     chown -R ceph:ceph \"$TMP\"\n\
                     install -d -o ceph -g ceph -m 0750 \"/var/lib/ceph/mon/ceph-$HOST\"\n\
                     ceph-mon --mkfs -i \"$HOST\" --monmap \"$TMP/monmap\" --keyring \"$TMP/mon.keyring\" --setuser ceph --setgroup ceph\n\
                     svc_start mon \"$HOST\"\n\
                     wait_for {wait} in_quorum \"$HOST\"\n",
                    sq(&format!("adding monitor {} on {mon_ip}", p.host)),
                    sq(&mon_ip.to_string()),
                ));
            }
            if *mgr {
                s.push_str("say 'adding a standby manager'\nstart_mgr\n");
            }
            s.push_str("say 'joined'\nceph -s\n");
        }
    }
    s
}

fn run_script(kind: &str, target: &str, who: &str, script: &str) -> Result<String, ApiError> {
    let dir = data_dir().join("ceph");
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::Internal(format!("{}: {e}", dir.display())))?;
    let path = dir.join(format!(
        "{kind}-{}.sh",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%.3f")
    ));
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o700)
            .open(&path)
            .map_err(|e| ApiError::Internal(format!("{}: {e}", path.display())))?;
        f.write_all(script.as_bytes())
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    }
    tasks::spawn_command(
        kind,
        target,
        who,
        "sh",
        &[path.to_string_lossy().to_string()],
        &[],
    )
}

fn need_portage() -> Result<(), ApiError> {
    if which("emerge").is_none() || !rooted("var/db/pkg").is_dir() {
        return Err(ApiError::NotAvailable {
            reason: "portage-missing",
            message: "Ceph is installed through Portage, which this node does not have.".into(),
            hint: "Install Ceph with the distribution's own packages; Horcrux then manages the cluster from Storage > Ceph.".into(),
        });
    }
    Ok(())
}

fn ceph_emerge_args() -> Vec<String> {
    crate::api::system_admin::packages::emerge_args(&["--update", "--noreplace"], &[CEPH_ATOM.into()])
}

async fn releases() -> Json<CephReleases> {
    Json(catalog(&sysroot(), pt::portage_arch()))
}

/// Check the request against what the node has; -> the release to pin.
pub fn check_install(cat: &CephReleases, major: u64, b: &Bootstrap) -> Result<CephRelease, ApiError> {
    let rel = cat
        .releases
        .iter()
        .find(|r| r.major == major)
        .cloned()
        .ok_or_else(|| unavailable(major, cat))?;
    if let Some(im) = cat.installed_major {
        if im > major {
            return Err(ApiError::Conflict(format!(
                "Ceph {} is installed: Ceph cannot be downgraded to {}",
                release_label(im),
                release_label(major)
            )));
        }
        if im < major {
            return Err(ApiError::Conflict(format!(
                "Ceph {} is installed: move to {} with the upgrade (POST /api/ceph/upgrade), which restarts the daemons in Ceph's order",
                release_label(im),
                release_label(major)
            )));
        }
    }
    if !matches!(b, Bootstrap::None) && cat.configured {
        return Err(ApiError::Conflict(
            "this node already has a Ceph configuration (/etc/ceph/ceph.conf): it is part of a cluster".into(),
        ));
    }
    Ok(rel)
}

async fn install(State(state): State<Arc<AppState>>, u: AuthExt, Json(r): Json<InstallReq>) -> Res<serde_json::Value> {
    need_portage()?;
    let major = parse_release(&r.release).map_err(ApiError::ValidationError)?;
    let root = sysroot();
    let arch = pt::portage_arch();
    let cat = catalog(&root, arch);
    let rel = check_install(&cat, major, &r.bootstrap)?;
    crate::api::system_admin::packages::guard_one_at_a_time()?;
    let host = short_hostname();
    if !valid_mon_id(&host) {
        return Err(ApiError::Conflict(format!(
            "this node's host name '{host}' cannot name a Ceph monitor (letters, digits and '-')"
        )));
    }
    let step = match &r.bootstrap {
        Bootstrap::None => PlanStep::None,
        Bootstrap::New(n) => {
            let net = parse_cidr(&n.public_network).map_err(ApiError::ValidationError)?;
            if let Some(cn) = n.cluster_network.as_deref().filter(|s| !s.trim().is_empty()) {
                parse_cidr(cn).map_err(ApiError::ValidationError)?;
            }
            if n.pool_size == 0 || n.pool_min_size == 0 || n.pool_min_size > n.pool_size {
                return Err(ApiError::ValidationError(
                    "pool_size and pool_min_size must be at least 1, and pool_min_size at most pool_size".into(),
                ));
            }
            let mon_ip = choose_ip(n.mon_ip.as_deref(), net)?;
            let fsid = uuid::Uuid::new_v4().to_string();
            PlanStep::New {
                conf: ceph_conf(&fsid, &host, mon_ip, n),
                fsid,
                mon_ip,
                ports: n.ports.clone(),
                wait: n.wait_secs,
            }
        }
        Bootstrap::Join(j) => {
            let m = fetch_join_material(&state, j).await?;
            let net = conf_value(&m.conf, "public_network")
                .ok_or_else(|| ApiError::Conflict("the peer's ceph.conf has no public_network".into()))
                .and_then(|n| parse_cidr(&n).map_err(ApiError::Conflict))?;
            let mon_ip = choose_ip(j.mon_ip.as_deref(), net)?;
            write_join_material(&root, &m)?;
            PlanStep::Join {
                peer: j.peer.clone(),
                mon_ip,
                mon: j.mon,
                mgr: j.mgr,
                wait: j.wait_secs,
            }
        }
    };
    write_pins(&root, &rel, arch)?;
    let plan = InstallPlan {
        major,
        emerge: ceph_emerge_args(),
        host,
        step,
    };
    let upid = run_script("ceph-install", &release_label(major), &user_of(&u), &install_script(&plan))?;
    Ok(Json(serde_json::json!({"upid": upid, "release": major, "name": release_label(major)})))
}

/// Pin `major` and install or update its packages (no cluster bootstrap):
/// what the declarative `ceph.release` applies. -> the task's UPID.
pub fn start_packages(major: u64, who: &str) -> Result<String, ApiError> {
    need_portage()?;
    let root = sysroot();
    let arch = pt::portage_arch();
    let cat = catalog(&root, arch);
    let rel = match cat.installed_major {
        Some(im) if im < major => check_upgrade(&cat, major, &["packages".into()])?,
        _ => check_install(&cat, major, &Bootstrap::None)?,
    };
    crate::api::system_admin::packages::guard_one_at_a_time()?;
    write_pins(&root, &rel, arch)?;
    let script = match cat.installed_major {
        Some(im) if im < major => upgrade_script(
            major,
            &["packages".into()],
            &ceph_emerge_args(),
            &short_hostname(),
            default_wait(),
        ),
        _ => install_script(&InstallPlan {
            major,
            emerge: ceph_emerge_args(),
            host: short_hostname(),
            step: PlanStep::None,
        }),
    };
    run_script("ceph-install", &release_label(major), who, &script)
}

fn choose_ip(given: Option<&str>, net: (IpAddr, u8)) -> Result<IpAddr, ApiError> {
    match given.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => {
            let ip: IpAddr = s
                .parse()
                .map_err(|_| ApiError::ValidationError(format!("'{s}' is not an IP address")))?;
            if !in_network(ip, net) {
                return Err(ApiError::ValidationError(format!(
                    "{ip} is not in the public network {}/{}",
                    net.0, net.1
                )));
            }
            Ok(ip)
        }
        None => pick_ip(&local_addrs(), net).ok_or_else(|| {
            ApiError::ValidationError(format!(
                "this node has no address in the public network {}/{}: give mon_ip or pick the right network",
                net.0, net.1
            ))
        }),
    }
}

/// `key = value` from the [global] part of a ceph.conf (spaces or underscores in the key).
pub fn conf_value(conf: &str, key: &str) -> Option<String> {
    let norm = |k: &str| k.trim().replace(' ', "_");
    let mut global = true;
    for l in conf.lines() {
        let l = l.trim();
        if l.starts_with('[') {
            global = l == "[global]";
            continue;
        }
        if !global || l.starts_with(['#', ';']) {
            continue;
        }
        if let Some((k, v)) = l.split_once('=') {
            if norm(k) == key {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

// ----------------------------------------------------------------- join

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinMaterial {
    pub fsid: String,
    pub conf: String,
    pub admin_keyring: String,
}

async fn join_material() -> Res<JoinMaterial> {
    let root = sysroot();
    let conf = std::fs::read_to_string(root.join("etc/ceph/ceph.conf")).map_err(|_| ApiError::NotAvailable {
        reason: "ceph-not-configured",
        message: "This node is not a Ceph cluster member (no /etc/ceph/ceph.conf).".into(),
        hint: "Join through a node that runs a Ceph monitor, or create the cluster first.".into(),
    })?;
    let admin_keyring = std::fs::read_to_string(root.join("etc/ceph/ceph.client.admin.keyring"))
        .map_err(|_| ApiError::Conflict("this node has no Ceph admin keyring to share".into()))?;
    let fsid = conf_value(&conf, "fsid").unwrap_or_default();
    Ok(Json(JoinMaterial {
        fsid,
        conf,
        admin_keyring,
    }))
}

async fn fetch_join_material(state: &Arc<AppState>, j: &JoinCluster) -> Result<JoinMaterial, ApiError> {
    let peer = j.peer.trim();
    if peer.is_empty() || peer.contains(['/', ' ', '@']) {
        return Err(ApiError::ValidationError(
            "peer must be a host name or address, optionally with :port".into(),
        ));
    }
    let base = if peer.matches(':').count() == 1 && !peer.starts_with('[') {
        format!("https://{peer}")
    } else {
        format!("https://{peer}:{}", state.config.server.port)
    };
    // peers serve HTTPS with their own node-local CA (as the cluster join does)
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(30))
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let login = client
        .post(format!("{base}/api/auth/login"))
        .json(&serde_json::json!({"username": j.username, "password": j.password}))
        .send()
        .await
        .map_err(|e| ApiError::BadRequest(format!("cannot reach {peer}: {e}")))?;
    if !login.status().is_success() {
        return Err(ApiError::BadRequest(format!(
            "login at {peer} failed ({}): check the administrator user name and password",
            login.status()
        )));
    }
    let body: serde_json::Value = login
        .json()
        .await
        .map_err(|e| ApiError::BadRequest(format!("unexpected login reply: {e}")))?;
    let token = body
        .get("token")
        .or_else(|| body.get("ticket"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| ApiError::BadRequest("the login did not return a token".into()))?
        .to_string();
    let r = client
        .post(format!("{base}/api/ceph/join-material"))
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| ApiError::BadRequest(format!("cannot reach {peer}: {e}")))?;
    if !r.status().is_success() {
        let st = r.status();
        let t = r.text().await.unwrap_or_default();
        return Err(ApiError::Conflict(format!(
            "{peer} did not hand out its Ceph configuration ({st}): {}",
            t.chars().take(300).collect::<String>()
        )));
    }
    let m: JoinMaterial = r
        .json()
        .await
        .map_err(|e| ApiError::BadRequest(format!("unexpected join-material reply: {e}")))?;
    if m.fsid.is_empty() || !m.admin_keyring.contains("client.admin") {
        return Err(ApiError::Conflict(format!(
            "{peer} answered without a cluster id or admin keyring"
        )));
    }
    Ok(m)
}

fn write_join_material(root: &Path, m: &JoinMaterial) -> Result<(), ApiError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = root.join("etc/ceph");
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::Internal(format!("{}: {e}", dir.display())))?;
    std::fs::write(dir.join("ceph.conf"), &m.conf).map_err(|e| ApiError::Internal(e.to_string()))?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(dir.join("ceph.client.admin.keyring"))
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    f.write_all(m.admin_keyring.as_bytes())
        .map_err(|e| ApiError::Internal(e.to_string()))
}

// --------------------------------------------------------------- upgrade

/// Ceph's upgrade order.
pub const UPGRADE_STEPS: &[&str] = &[
    "packages",
    "restart-mon",
    "restart-mgr",
    "restart-osd",
    "restart-mds",
    "restart-rgw",
    "finish",
];

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DaemonVersions {
    /// daemon type -> (major version, count)
    pub by_type: Vec<(String, Vec<(u64, u64)>)>,
}

/// `ceph versions -f json`: `{"mon": {"ceph version 20.2.5 (...) tentacle (stable)": 3}, ...}`.
pub fn parse_versions(v: &serde_json::Value) -> DaemonVersions {
    let mut out = DaemonVersions::default();
    let Some(o) = v.as_object() else {
        return out;
    };
    for (kind, m) in o {
        if kind == "overall" {
            continue;
        }
        let mut list: Vec<(u64, u64)> = Vec::new();
        for (text, n) in m.as_object().into_iter().flatten() {
            let major = text
                .strip_prefix("ceph version ")
                .map(|r| pt::major_minor(r).0)
                .unwrap_or(0);
            let n = n.as_u64().unwrap_or(0);
            match list.iter_mut().find(|(m, _)| *m == major) {
                Some(e) => e.1 += n,
                None => list.push((major, n)),
            }
        }
        list.sort();
        out.by_type.push((kind.clone(), list));
    }
    out.by_type.sort_by_key(|(k, _)| {
        ["mon", "mgr", "osd", "mds", "rgw"]
            .iter()
            .position(|x| *x == k.as_str())
            .unwrap_or(9)
    });
    out
}

/// The first step still to do for `target`: a daemon type still running an
/// older release must be restarted before the next type (mon, mgr, osd, mds,
/// rgw), and `finish` once everything runs the target.
pub fn next_step(d: &DaemonVersions, installed_major: Option<u64>, target: u64) -> &'static str {
    if installed_major.is_some_and(|m| m < target) {
        return "packages";
    }
    for (kind, step) in [
        ("mon", "restart-mon"),
        ("mgr", "restart-mgr"),
        ("osd", "restart-osd"),
        ("mds", "restart-mds"),
        ("rgw", "restart-rgw"),
    ] {
        if d
            .by_type
            .iter()
            .any(|(k, l)| k == kind && l.iter().any(|(m, _)| *m < target))
        {
            return step;
        }
    }
    "finish"
}

#[derive(Debug, Clone, Serialize)]
pub struct UpgradeStatus {
    pub installed: Option<String>,
    pub installed_major: Option<u64>,
    pub pinned: Option<u64>,
    pub daemons: Option<DaemonVersions>,
    /// newest release a cluster on the installed one may move to
    pub targets: Vec<u64>,
    /// next step for the pinned (or newest allowed) target
    pub next_step: Option<String>,
    pub error: Option<String>,
}

async fn ceph_json(args: &[&str]) -> Result<serde_json::Value, String> {
    let mut a = args.to_vec();
    a.extend(["--connect-timeout", "8", "-f", "json"]);
    let out = crate::api::system_admin::run_secs("ceph", &a, 30).await?;
    serde_json::from_str(&out).map_err(|e| e.to_string())
}

async fn upgrade_status() -> Json<UpgradeStatus> {
    let cat = catalog(&sysroot(), pt::portage_arch());
    let (daemons, error) = if which("ceph").is_some() {
        match ceph_json(&["versions"]).await {
            Ok(v) => (Some(parse_versions(&v)), None),
            Err(e) => (None, Some(e)),
        }
    } else {
        (None, Some("Ceph is not installed on this node".into()))
    };
    let targets: Vec<u64> = match cat.installed_major {
        Some(im) => cat
            .releases
            .iter()
            .map(|r| r.major)
            .filter(|m| *m > im && *m <= im + MAX_UPGRADE_SPAN)
            .collect(),
        None => vec![],
    };
    let target = cat
        .pinned
        .filter(|p| cat.installed_major.is_some_and(|im| *p > im))
        .or_else(|| targets.first().copied());
    let next_step = match (target, &daemons) {
        (Some(t), Some(d)) => Some(next_step(d, cat.installed_major, t).to_string()),
        (Some(_), None) => Some("packages".into()),
        _ => None,
    };
    Json(UpgradeStatus {
        installed: cat.installed,
        installed_major: cat.installed_major,
        pinned: cat.pinned,
        daemons,
        targets,
        next_step,
        error,
    })
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpgradeReq {
    pub release: String,
    /// default: every step in order (a single-node cluster); on several nodes
    /// run `packages` everywhere, then each restart step on every node
    #[serde(default)]
    pub steps: Vec<String>,
    #[serde(default = "default_wait")]
    pub wait_secs: u64,
}

pub fn check_upgrade(cat: &CephReleases, major: u64, steps: &[String]) -> Result<CephRelease, ApiError> {
    for s in steps {
        if !UPGRADE_STEPS.contains(&s.as_str()) {
            return Err(ApiError::ValidationError(format!(
                "unknown upgrade step '{s}' (steps: {})",
                UPGRADE_STEPS.join(", ")
            )));
        }
    }
    let im = cat
        .installed_major
        .ok_or_else(|| ApiError::Conflict("Ceph is not installed on this node: install it first".into()))?;
    let rel = cat
        .releases
        .iter()
        .find(|r| r.major == major)
        .cloned()
        .ok_or_else(|| unavailable(major, cat))?;
    if major < im {
        return Err(ApiError::Conflict(format!(
            "Ceph cannot be downgraded ({} is installed)",
            release_label(im)
        )));
    }
    if major > im + MAX_UPGRADE_SPAN {
        return Err(ApiError::Conflict(format!(
            "Ceph upgrades at most {MAX_UPGRADE_SPAN} releases at a time: go from {} to {} first",
            release_label(im),
            release_label(im + MAX_UPGRADE_SPAN)
        )));
    }
    Ok(rel)
}

pub fn upgrade_script(major: u64, steps: &[String], emerge: &[String], host: &str, wait: u64) -> String {
    let mut s = String::from(SCRIPT_HEAD);
    s.push_str(&format!("HOST={}\n", sq(host)));
    let all: Vec<String> = if steps.is_empty() {
        UPGRADE_STEPS.iter().map(|x| x.to_string()).collect()
    } else {
        UPGRADE_STEPS
            .iter()
            .filter(|x| steps.iter().any(|s| s == *x))
            .map(|x| x.to_string())
            .collect()
    };
    let codename = release_name(major).unwrap_or("").to_ascii_lowercase();
    for step in &all {
        match step.as_str() {
            "packages" => s.push_str(&format!(
                "say {}\n\
                 if [ -f /etc/ceph/ceph.conf ]; then ceph --connect-timeout 8 osd set noout || true; fi\n\
                 emerge {}\nceph --version\n",
                sq(&format!("updating {CEPH_ATOM} to release {}", release_label(major))),
                emerge.iter().map(|a| sq(a)).collect::<Vec<_>>().join(" ")
            )),
            "restart-mon" => s.push_str(&format!(
                "svc_restart_all mon\nwait_for {wait} in_quorum \"$HOST\" || [ ! -d \"/var/lib/ceph/mon/ceph-$HOST\" ]\n"
            )),
            "restart-mgr" => s.push_str("svc_restart_all mgr\n"),
            "restart-osd" => s.push_str(&format!(
                "svc_restart_all osd\nwait_for {wait} sh -c 'ceph --connect-timeout 5 pg stat | grep -qv \"peering\\|down\\|unknown\"'\n"
            )),
            "restart-mds" => s.push_str("svc_restart_all mds\n"),
            "restart-rgw" => s.push_str("svc_restart_all rgw\n"),
            "finish" => {
                if !codename.is_empty() {
                    s.push_str(&format!(
                        "say {}\nceph --connect-timeout 8 osd require-osd-release {}\n",
                        sq(&format!("requiring {codename} on every OSD")),
                        sq(&codename)
                    ));
                }
                s.push_str("ceph --connect-timeout 8 osd unset noout\nceph versions\nceph -s\n");
            }
            _ => {}
        }
    }
    s.push_str("say 'upgrade step(s) done'\n");
    s
}

async fn upgrade(u: AuthExt, Json(r): Json<UpgradeReq>) -> Res<serde_json::Value> {
    need_portage()?;
    let major = parse_release(&r.release).map_err(ApiError::ValidationError)?;
    let root = sysroot();
    let arch = pt::portage_arch();
    let cat = catalog(&root, arch);
    let rel = check_upgrade(&cat, major, &r.steps)?;
    if r.steps.iter().any(|s| s == "finish") || r.steps.is_empty() {
        // finishing while daemons still run the old release would lock them out
        if let Ok(v) = ceph_json(&["versions"]).await {
            let d = parse_versions(&v);
            let n = next_step(&d, Some(major), major);
            if !r.steps.is_empty() && n != "finish" {
                return Err(ApiError::Conflict(format!(
                    "not every daemon runs {} yet: the next step is {n} (on every node)",
                    release_label(major)
                )));
            }
        }
    }
    crate::api::system_admin::packages::guard_one_at_a_time()?;
    write_pins(&root, &rel, arch)?;
    let script = upgrade_script(major, &r.steps, &ceph_emerge_args(), &short_hostname(), r.wait_secs);
    let upid = run_script("ceph-upgrade", &release_label(major), &user_of(&u), &script)?;
    Ok(Json(serde_json::json!({"upid": upid, "release": major})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::system_admin::portage_tree::tests::{ebuild, vdb};

    fn tree(arch: &str) -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        ebuild(t.path(), "gentoo", CEPH_ATOM, "20.2.5", &format!("KEYWORDS=\"~{arch} ~arm64\"\n"));
        ebuild(t.path(), "gentoo", CEPH_ATOM, "20.1.1-r1", &format!("KEYWORDS=\"{arch} ppc64\"\n"));
        ebuild(t.path(), "horcrux", CEPH_ATOM, "21.2.0", &format!("KEYWORDS=\"~{arch}\"\n"));
        ebuild(t.path(), "gentoo", CEPH_ATOM, "19.2.7", "KEYWORDS=\"~ppc64\"\n");
        std::fs::create_dir_all(t.path().join("var/db/pkg")).unwrap();
        t
    }

    #[test]
    fn release_names_and_parsing() {
        assert_eq!(parse_release("umbrella").unwrap(), 21);
        assert_eq!(parse_release("Tentacle").unwrap(), 20);
        assert_eq!(parse_release("21.2.0").unwrap(), 21);
        assert_eq!(parse_release("v22").unwrap(), 22);
        assert!(parse_release("latest").is_err());
        assert!(parse_release("0").is_err());
        assert_eq!(release_label(21), "Umbrella (21)");
        assert_eq!(release_label(23), "Ceph 23");
    }

    #[test]
    fn the_catalog_groups_by_release_for_this_arch() {
        let t = tree("amd64");
        let c = catalog(t.path(), "amd64");
        assert_eq!(c.releases.iter().map(|r| r.major).collect::<Vec<_>>(), [21, 20]);
        assert_eq!(c.recommended, Some(21));
        assert_eq!(c.releases[0].repo, "horcrux");
        assert_eq!(c.releases[1].newest, "20.2.5");
        assert_eq!(c.releases[1].versions, ["20.2.5", "20.1.1-r1"]);
        assert_eq!(c.releases[1].keyword, Keyword::Testing);
        assert!(c.installed.is_none());
        // arm64 has no Umbrella here
        let c = catalog(t.path(), "arm64");
        assert_eq!(c.releases.iter().map(|r| r.major).collect::<Vec<_>>(), [20]);
    }

    #[test]
    fn pins_keep_ceph_inside_the_release() {
        let t = tree("amd64");
        let c = catalog(t.path(), "amd64");
        let (kw, mask) = pins(&c.releases[0], "amd64");
        assert!(mask.contains("<sys-cluster/ceph-21\n>=sys-cluster/ceph-22\n"), "{mask}");
        assert!(mask.contains("# release: 21\n"));
        assert_eq!(kw.unwrap().lines().last(), Some("=sys-cluster/ceph-21* ~amd64"));
        write_pins(t.path(), &c.releases[0], "amd64").unwrap();
        assert_eq!(catalog(t.path(), "amd64").pinned, Some(21));
        let stable = CephRelease {
            keyword: Keyword::Stable,
            ..c.releases[1].clone()
        };
        assert!(pins(&stable, "amd64").0.is_none());
    }

    #[test]
    fn install_checks() {
        let t = tree("amd64");
        let c = catalog(t.path(), "amd64");
        assert!(check_install(&c, 21, &Bootstrap::None).is_ok());
        match check_install(&c, 19, &Bootstrap::None).unwrap_err() {
            ApiError::NotAvailable { reason, hint, .. } => {
                assert_eq!(reason, "ceph-release-unavailable");
                assert!(hint.contains("Umbrella (21)"), "{hint}");
            }
            e => panic!("{e:?}"),
        }
        vdb(t.path(), CEPH_ATOM, "20.2.5", "");
        let c = catalog(t.path(), "amd64");
        assert!(matches!(check_install(&c, 21, &Bootstrap::None), Err(ApiError::Conflict(m)) if m.contains("upgrade")));
        assert!(check_install(&c, 20, &Bootstrap::None).is_ok());
        std::fs::create_dir_all(t.path().join("etc/ceph")).unwrap();
        std::fs::write(t.path().join("etc/ceph/ceph.conf"), "[global]\n").unwrap();
        let c = catalog(t.path(), "amd64");
        let n = Bootstrap::New(serde_json::from_value(serde_json::json!({"public_network": "10.0.0.0/24"})).unwrap());
        assert!(matches!(check_install(&c, 20, &n), Err(ApiError::Conflict(_))));
    }

    #[test]
    fn networks_and_addresses() {
        let n = parse_cidr("10.0.2.0/24").unwrap();
        let a: Vec<IpAddr> = ["127.0.0.1", "192.168.5.4", "10.0.2.15"].iter().map(|s| s.parse().unwrap()).collect();
        assert_eq!(pick_ip(&a, n), Some("10.0.2.15".parse().unwrap()));
        assert!(in_network("fd00::5".parse().unwrap(), parse_cidr("fd00::/64").unwrap()));
        assert!(!in_network("10.0.3.1".parse().unwrap(), n));
        assert!(parse_cidr("10.0.0.0").is_err());
        assert!(parse_cidr("10.0.0.0/33").is_err());
        assert_eq!(mon_addrv("10.0.2.15".parse().unwrap(), &MonPorts::default()), "[v2:10.0.2.15:3300,v1:10.0.2.15:6789]");
        assert_eq!(mon_addrv("fd00::5".parse().unwrap(), &MonPorts::default()), "[v2:[fd00::5]:3300,v1:[fd00::5]:6789]");
        assert!(valid_mon_id("node-1") && !valid_mon_id("a b") && !valid_mon_id(""));
        assert_eq!(sq("it's"), r"'it'\''s'");
    }

    #[test]
    fn the_bootstrap_request_reads_from_json() {
        let r: InstallReq = serde_json::from_value(serde_json::json!({"release": "umbrella"})).unwrap();
        assert_eq!(r.bootstrap, Bootstrap::None);
        let r: InstallReq = serde_json::from_value(serde_json::json!({
            "release": "21", "bootstrap": {"mode": "new", "public_network": "10.0.0.0/24"}})).unwrap();
        match r.bootstrap {
            Bootstrap::New(n) => {
                assert_eq!(n.pool_size, 3);
                assert_eq!(n.pool_min_size, 2);
                assert_eq!(n.ports, MonPorts::default());
                assert_eq!(n.wait_secs, 180);
            }
            b => panic!("{b:?}"),
        }
        let r: InstallReq = serde_json::from_value(serde_json::json!({
            "release": "21", "bootstrap": {"mode": "join", "peer": "10.0.0.1", "username": "root", "password": "x"}})).unwrap();
        assert!(matches!(r.bootstrap, Bootstrap::Join(ref j) if j.mon && j.mgr));
    }

    #[test]
    fn the_new_cluster_script_creates_mon_and_mgr() {
        let n: NewCluster = serde_json::from_value(serde_json::json!({"public_network": "10.0.2.0/24", "cluster_network": "10.9.0.0/24"})).unwrap();
        let ip: IpAddr = "10.0.2.15".parse().unwrap();
        let conf = ceph_conf("f-1", "node1", ip, &n);
        assert!(conf.contains("fsid = f-1\n"));
        assert!(conf.contains("mon_host = [v2:10.0.2.15:3300,v1:10.0.2.15:6789]\n"));
        assert!(conf.contains("cluster_network = 10.9.0.0/24\n"));
        assert!(conf.contains("osd_pool_default_size = 3\n"));
        assert_eq!(conf_value(&conf, "public_network").as_deref(), Some("10.0.2.0/24"));
        assert_eq!(conf_value("[global]\npublic network = 1.2.3.0/24\n[mon.a]\nfsid = x\n", "public_network").as_deref(), Some("1.2.3.0/24"));
        let s = install_script(&InstallPlan {
            major: 21,
            emerge: vec!["--ask=n".into(), CEPH_ATOM.into()],
            host: "node1".into(),
            step: PlanStep::New { fsid: "f-1".into(), conf, mon_ip: ip, ports: MonPorts::default(), wait: 60 },
        });
        assert!(s.starts_with("#!/bin/sh\n"));
        assert!(s.contains("emerge '--ask=n' 'sys-cluster/ceph'\n"));
        assert!(s.contains("monmaptool --create --addv \"$HOST\" '[v2:10.0.2.15:3300,v1:10.0.2.15:6789]' --fsid 'f-1'"));
        assert!(s.contains("ceph-mon --mkfs -i \"$HOST\""));
        assert!(s.contains("svc_start mon \"$HOST\"\nwait_for 60 in_quorum \"$HOST\""));
        assert!(s.contains("start_mgr\n"));
        // both init systems are handled by the script itself
        assert!(s.contains("systemctl enable \"ceph-$1@$2.service\""));
        assert!(s.contains("rc-update add \"ceph-$1.$2\" default"));
    }

    #[test]
    fn the_join_script_adds_a_monitor_with_its_address() {
        let s = install_script(&InstallPlan {
            major: 21,
            emerge: vec![CEPH_ATOM.into()],
            host: "node2".into(),
            step: PlanStep::Join { peer: "10.0.2.15".into(), mon_ip: "10.0.2.16".parse().unwrap(), mon: true, mgr: false, wait: 30 },
        });
        assert!(s.contains("ceph auth get client.bootstrap-osd"));
        assert!(s.contains("ceph mon getmap -o \"$TMP/monmap\""));
        assert!(s.contains("public_addr = %s\\n' \"$HOST\" '10.0.2.16'"));
        assert!(!s.contains("start_mgr\nsay"));
    }

    #[test]
    fn upgrades_follow_cephs_order() {
        let v = serde_json::json!({
            "mon": {"ceph version 21.2.0 (abc) umbrella (stable)": 2, "ceph version 20.2.5 (def) tentacle (stable)": 1},
            "mgr": {"ceph version 20.2.5 (def) tentacle (stable)": 3},
            "osd": {"ceph version 20.2.5 (def) tentacle (stable)": 3},
            "overall": {}
        });
        let d = parse_versions(&v);
        assert_eq!(d.by_type[0], ("mon".to_string(), vec![(20, 1), (21, 2)]));
        assert_eq!(next_step(&d, Some(20), 21), "packages");
        assert_eq!(next_step(&d, Some(21), 21), "restart-mon");
        let v = serde_json::json!({"mon": {"ceph version 21.2.0 (a) umbrella (stable)": 3},
            "mgr": {"ceph version 21.2.0 (a) umbrella (stable)": 3},
            "osd": {"ceph version 20.2.5 (d) tentacle (stable)": 3}});
        assert_eq!(next_step(&parse_versions(&v), Some(21), 21), "restart-osd");
        let v = serde_json::json!({"mon": {"ceph version 21.2.0 (a) umbrella (stable)": 3}});
        assert_eq!(next_step(&parse_versions(&v), Some(21), 21), "finish");
        let s = upgrade_script(21, &["restart-mon".into(), "packages".into()], &[CEPH_ATOM.into()], "n1", 30);
        let (p, m) = (s.find("emerge 'sys-cluster/ceph'").unwrap(), s.find("svc_restart_all mon").unwrap());
        assert!(p < m, "packages before the monitor restart");
        assert!(!s.contains("require-osd-release"));
        let s = upgrade_script(21, &[], &[CEPH_ATOM.into()], "n1", 30);
        assert!(s.contains("osd require-osd-release 'umbrella'"));
        assert!(s.find("svc_restart_all osd").unwrap() < s.find("svc_restart_all mds").unwrap());
    }

    #[test]
    fn upgrade_checks() {
        let t = tree("amd64");
        let c = catalog(t.path(), "amd64");
        assert!(matches!(check_upgrade(&c, 21, &[]), Err(ApiError::Conflict(m)) if m.contains("not installed")));
        vdb(t.path(), CEPH_ATOM, "20.2.5", "");
        let c = catalog(t.path(), "amd64");
        assert!(check_upgrade(&c, 21, &["packages".into()]).is_ok());
        assert!(matches!(check_upgrade(&c, 21, &["reboot".into()]), Err(ApiError::ValidationError(_))));
        let mut c2 = c.clone();
        c2.installed_major = Some(17);
        c2.releases.push(CephRelease { major: 17, ..c.releases[0].clone() });
        assert!(matches!(check_upgrade(&c2, 21, &[]), Err(ApiError::Conflict(m)) if m.contains("at most 2")));
    }
}
