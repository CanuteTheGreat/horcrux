//! NVMe over Fabrics (NVMe/TCP, NVMe/RDMA) initiator side of the SAN:
//! typed targets by subsystem NQN, discovery through every listed
//! discovery controller (`nvme discover -o json`), `nvme connect` on every
//! advertised path, and the path view read straight from sysfs
//! (`/sys/class/nvme`, `/sys/class/nvme-subsystem`, `/sys/block/*/ana_state`),
//! so it works with every nvme-cli version and without nvme-cli for reads.
//!
//! Multipath: the kernel's native NVMe multipath (`nvme_core.multipath=Y`,
//! the default on Gentoo and most distribution kernels) joins the paths of
//! a namespace into one head device (`/dev/nvme0n1`); Horcrux sets the
//! subsystem's I/O policy ([`NvmeIoPolicy`]). With native multipath off,
//! every path is its own block device and dm-multipath maps them by WWID
//! like iSCSI LUNs.

use super::iscsi::{Portal, Run};
use super::paths::{LunView, PathState, PathView};
use super::{valid_id, yes};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// IANA port of NVMe-oF I/O controllers (NVMe/TCP and RDMA).
pub const DEFAULT_IO_PORT: u16 = 4420;
/// IANA port of NVMe-oF discovery controllers.
pub const DEFAULT_DISCOVERY_PORT: u16 = 8009;
/// Where nvme-cli and the kernel read this host's NQN.
pub const HOSTNQN_PATH: &str = "/etc/nvme/hostnqn";

/// Fabric transport.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum NvmeTransport {
    #[default]
    Tcp,
    Rdma,
}

impl NvmeTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            NvmeTransport::Tcp => "tcp",
            NvmeTransport::Rdma => "rdma",
        }
    }
    /// The kernel module `nvme connect` needs.
    pub fn module(self) -> &'static str {
        match self {
            NvmeTransport::Tcp => "nvme_tcp",
            NvmeTransport::Rdma => "nvme_rdma",
        }
    }
}

impl fmt::Display for NvmeTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Native multipath I/O policy of a subsystem
/// (`/sys/class/nvme-subsystem/<s>/iopolicy`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum NvmeIoPolicy {
    /// The path closest to the submitting CPU's NUMA node (kernel default).
    Numa,
    /// Spread I/O over every optimized path (the usual SAN choice).
    #[default]
    RoundRobin,
    /// The path with the fewest requests in flight (kernel 6.11+).
    QueueDepth,
}

impl NvmeIoPolicy {
    pub fn as_sysfs(self) -> &'static str {
        match self {
            NvmeIoPolicy::Numa => "numa",
            NvmeIoPolicy::RoundRobin => "round-robin",
            NvmeIoPolicy::QueueDepth => "queue-depth",
        }
    }
}

/// NVMe Qualified Name: `nqn.YYYY-MM.reverse-domain[:anything]`, or the
/// UUID form `nqn.2014-08.org.nvmexpress:uuid:<uuid>`; at most 223 bytes.
pub fn valid_nqn(s: &str) -> bool {
    if s.len() > 223 || s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let Some(rest) = s.strip_prefix("nqn.") else {
        return false;
    };
    let b = rest.as_bytes();
    b.len() >= 9
        && b[4] == b'-'
        && b[7] == b'.'
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[8].is_ascii_alphanumeric()
}

fn default_ctrl_loss_tmo() -> i32 {
    600
}
fn default_reconnect_delay() -> u32 {
    10
}

/// One NVMe-oF subsystem the cluster connects to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NvmeTargetSpec {
    pub id: String,
    /// Subsystem NQN.
    pub subnqn: String,
    #[serde(default)]
    pub transport: NvmeTransport,
    /// Discovery controllers (`host`, `host:port`, `[v6]:port`; default
    /// port 8009). With `use_advertised_addresses` off these are the I/O
    /// controllers to connect to instead (default port 4420).
    pub addresses: Vec<String>,
    /// Connect to every path the discovery log reports for `subnqn`
    /// (default true: that is what gives multipath its paths).
    #[serde(default = "yes")]
    pub use_advertised_addresses: bool,
    /// `--ctrl-loss-tmo`: seconds a lost controller keeps reconnecting
    /// before it is removed (default 600, the kernel's; -1 = forever).
    #[serde(default = "default_ctrl_loss_tmo")]
    pub ctrl_loss_tmo_secs: i32,
    /// `--reconnect-delay` (default 10).
    #[serde(default = "default_reconnect_delay")]
    pub reconnect_delay_secs: u32,
    /// Native multipath I/O policy for this subsystem.
    #[serde(default)]
    pub iopolicy: NvmeIoPolicy,
    /// Limit the target to some nodes (empty = every node).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Change counter (unix ms) for the cluster merge; newest wins.
    #[serde(default)]
    pub rev: u64,
    /// Tombstone, so a delete reaches nodes that were offline.
    #[serde(default)]
    pub deleted: bool,
}

impl NvmeTargetSpec {
    pub fn applies_to(&self, node: &str) -> bool {
        self.enabled
            && !self.deleted
            && (self.nodes.is_empty() || self.nodes.iter().any(|n| n == node))
    }

    fn default_port(&self) -> u16 {
        if self.use_advertised_addresses {
            DEFAULT_DISCOVERY_PORT
        } else {
            DEFAULT_IO_PORT
        }
    }

    /// The listed addresses, parsed with the right default port.
    pub fn parsed_addresses(&self) -> Result<Vec<Portal>, String> {
        self.addresses
            .iter()
            .map(|a| Portal::parse_default(a, self.default_port()))
            .collect()
    }

    pub fn validate(&mut self) -> Result<(), String> {
        self.id = self.id.trim().to_string();
        self.subnqn = self.subnqn.trim().to_string();
        if !valid_id(&self.id) {
            return Err(format!(
                "id {:?}: use letters, digits, '-' and '_' (max 64)",
                self.id
            ));
        }
        if !valid_nqn(&self.subnqn) {
            return Err(format!(
                "subnqn {:?} is not an NVMe qualified name (nqn.YYYY-MM.domain[:name])",
                self.subnqn
            ));
        }
        if self.subnqn == DISCOVERY_NQN {
            return Err("subnqn is the discovery NQN; give the storage subsystem's NQN".into());
        }
        if self.addresses.is_empty() {
            return Err("at least one address is needed".into());
        }
        let mut out = Vec::new();
        for p in self.parsed_addresses()? {
            let p = p.to_string();
            if !out.contains(&p) {
                out.push(p);
            }
        }
        self.addresses = out;
        if self.ctrl_loss_tmo_secs < -1 || self.ctrl_loss_tmo_secs > 86_400 {
            return Err("ctrl_loss_tmo_secs must be -1 (forever) or 0..86400".into());
        }
        if !(1..=3600).contains(&self.reconnect_delay_secs) {
            return Err("reconnect_delay_secs must be 1..3600".into());
        }
        for n in &self.nodes {
            if n.trim().is_empty() {
                return Err("nodes must not contain empty names".into());
            }
        }
        Ok(())
    }
}

/// The well-known discovery subsystem NQN.
pub const DISCOVERY_NQN: &str = "nqn.2014-08.org.nvmexpress.discovery";

/// One entry of a discovery log page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryRecord {
    pub trtype: String,
    pub traddr: String,
    pub trsvcid: String,
    pub subnqn: String,
    /// `nvme subsystem`, `discovery subsystem`, `referral`, ...
    pub subtype: String,
}

impl DiscoveryRecord {
    pub fn is_storage(&self) -> bool {
        // nvme-cli prints "nvme subsystem"; older ones "nvme"/"2".
        let s = self.subtype.to_ascii_lowercase();
        (s.contains("nvme") && !s.contains("discovery")) || s == "2"
    }
    pub fn address(&self) -> String {
        let port = self.trsvcid.trim().parse().unwrap_or(DEFAULT_IO_PORT);
        Portal {
            host: self.traddr.trim().to_string(),
            port,
        }
        .to_string()
    }
}

/// One subsystem as discovery reported it, with every path it advertised.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredSubsystem {
    pub subnqn: String,
    pub transport: String,
    /// `host:port` of every I/O controller.
    pub addresses: Vec<String>,
}

fn jstr(v: &serde_json::Value, k: &str) -> String {
    match v.get(k) {
        Some(serde_json::Value::String(s)) => s.trim().to_string(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// Parse `nvme discover -o json` (nvme-cli 1.x and 2.x: an object with a
/// `records` array; some versions wrap it in a list).
pub fn parse_discover_json(text: &str) -> Result<Vec<DiscoveryRecord>, String> {
    let v: serde_json::Value =
        serde_json::from_str(text.trim()).map_err(|e| format!("nvme discover json: {e}"))?;
    let objs: Vec<&serde_json::Value> = match &v {
        serde_json::Value::Array(a) => a.iter().collect(),
        o => vec![o],
    };
    let mut out = Vec::new();
    for o in objs {
        for r in o
            .get("records")
            .and_then(|r| r.as_array())
            .into_iter()
            .flatten()
        {
            out.push(DiscoveryRecord {
                trtype: jstr(r, "trtype"),
                traddr: jstr(r, "traddr"),
                trsvcid: jstr(r, "trsvcid"),
                subnqn: jstr(r, "subnqn"),
                subtype: jstr(r, "subtype"),
            });
        }
    }
    Ok(out)
}

/// Fold discovery records into subsystems (storage entries only, every
/// address once, sorted).
pub fn group_discovery(records: &[DiscoveryRecord]) -> Vec<DiscoveredSubsystem> {
    let mut m: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for r in records
        .iter()
        .filter(|r| r.is_storage() && !r.traddr.is_empty())
    {
        let a = m.entry((r.subnqn.clone(), r.trtype.clone())).or_default();
        let addr = r.address();
        if !a.contains(&addr) {
            a.push(addr);
        }
    }
    m.into_iter()
        .map(|((subnqn, transport), mut addresses)| {
            addresses.sort();
            DiscoveredSubsystem {
                subnqn,
                transport,
                addresses,
            }
        })
        .collect()
}

/// The I/O controller addresses to connect to for `t`.
pub fn connect_addresses(t: &NvmeTargetSpec, discovered: &[DiscoveredSubsystem]) -> Vec<Portal> {
    let mut out: Vec<Portal> = Vec::new();
    if t.use_advertised_addresses {
        for d in discovered
            .iter()
            .filter(|d| d.subnqn == t.subnqn && d.transport == t.transport.as_str())
        {
            for a in &d.addresses {
                if let Ok(p) = Portal::parse_default(a, DEFAULT_IO_PORT) {
                    if !out.contains(&p) {
                        out.push(p);
                    }
                }
            }
        }
    } else {
        out = t.parsed_addresses().unwrap_or_default();
    }
    out.sort();
    out
}

// ------------------------------------------------------------------ sysfs

/// One fabrics controller (`/sys/class/nvme/nvmeN`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Controller {
    /// `nvme0`.
    pub name: String,
    pub subsysnqn: String,
    /// `tcp`, `rdma`, `fc`, `pcie`, `loop`.
    pub transport: String,
    pub traddr: Option<String>,
    pub trsvcid: Option<String>,
    /// Local address of the connection (`src_addr` or `host_traddr`).
    pub local_address: Option<String>,
    /// `live`, `connecting`, `resetting`, `deleting`, `dead`, ...
    pub state: String,
}

impl Controller {
    pub fn address(&self) -> Option<String> {
        let h = self.traddr.as_ref()?;
        let port = self
            .trsvcid
            .as_deref()
            .and_then(|p| p.parse().ok())
            .unwrap_or(DEFAULT_IO_PORT);
        Some(
            Portal {
                host: h.clone(),
                port,
            }
            .to_string(),
        )
    }
    pub fn is_live(&self) -> bool {
        self.state == "live"
    }
}

/// Parse a controller's `address` file:
/// `traddr=10.0.0.1,trsvcid=4420,src_addr=10.0.0.20`.
pub fn parse_ctrl_address(s: &str) -> (Option<String>, Option<String>, Option<String>) {
    let (mut a, mut p, mut l) = (None, None, None);
    for kv in s.trim().split(',') {
        match kv.split_once('=') {
            Some(("traddr", v)) => a = Some(v.trim().to_string()),
            Some(("trsvcid", v)) => p = Some(v.trim().to_string()),
            Some(("src_addr", v)) => l = Some(v.trim().to_string()),
            Some(("host_traddr", v)) if l.is_none() => l = Some(v.trim().to_string()),
            _ => {}
        }
    }
    (a, p, l)
}

fn read(p: &Path) -> Option<String> {
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_string())
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// Every NVMe controller under `<root>/sys/class/nvme`.
pub fn scan_controllers(root: &Path) -> Vec<Controller> {
    let base = root.join("sys/class/nvme");
    names(&base)
        .into_iter()
        .filter(|n| n.starts_with("nvme"))
        .map(|n| {
            let d = base.join(&n);
            let (traddr, trsvcid, local_address) = read(&d.join("address"))
                .map(|s| parse_ctrl_address(&s))
                .unwrap_or_default();
            Controller {
                subsysnqn: read(&d.join("subsysnqn")).unwrap_or_default(),
                transport: read(&d.join("transport")).unwrap_or_default(),
                state: read(&d.join("state")).unwrap_or_default(),
                traddr,
                trsvcid,
                local_address,
                name: n,
            }
        })
        .collect()
}

/// A namespace of a subsystem as the host sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Namespace {
    /// Block device users open (`nvme0n1`): the multipath head with native
    /// multipath, the only path without.
    pub dev: String,
    pub nsid: Option<u32>,
    pub wwid: Option<String>,
    /// `(path device, controller)`: `nvme0c1n1` via `nvme1` with native
    /// multipath; `(dev, its controller)` without.
    pub paths: Vec<(String, String, Option<String>)>,
}

/// A subsystem with its namespaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subsystem {
    /// `nvme-subsys0`.
    pub name: String,
    pub subsysnqn: String,
    pub model: Option<String>,
    pub iopolicy: Option<String>,
    pub namespaces: Vec<Namespace>,
}

/// `nvme0c1n1` -> (subsystem instance 0, controller `nvme1`, nsid 1);
/// `nvme1n1` -> (1, None, 1).
pub fn split_dev(dev: &str) -> Option<(u32, Option<String>, u32)> {
    let rest = dev.strip_prefix("nvme")?;
    let (head, ns) = rest.rsplit_once('n')?;
    let ns: u32 = ns.parse().ok()?;
    match head.split_once('c') {
        Some((s, c)) => Some((
            s.parse().ok()?,
            Some(format!("nvme{}", c.parse::<u32>().ok()?)),
            ns,
        )),
        None => Some((head.parse().ok()?, None, ns)),
    }
}

/// Subsystems under `<root>/sys/class/nvme-subsystem`, with each
/// namespace's paths (path devices from `<root>/sys/block`).
pub fn scan_subsystems(root: &Path) -> Vec<Subsystem> {
    let base = root.join("sys/class/nvme-subsystem");
    let block = root.join("sys/block");
    let all_block = names(&block);
    let mut out = Vec::new();
    for s in names(&base) {
        let d = base.join(&s);
        let entries = names(&d);
        let ctrls: HashSet<&str> = entries
            .iter()
            .filter(|e| split_dev(e).is_none() && e.starts_with("nvme") && !e.contains('-'))
            .map(String::as_str)
            .collect();
        let mut namespaces = Vec::new();
        for e in entries
            .iter()
            .filter(|e| matches!(split_dev(e), Some((_, None, _))))
        {
            let (inst, _, nsid) = split_dev(e).unwrap();
            let wwid = read(&d.join(e).join("wwid")).or_else(|| read(&block.join(e).join("wwid")));
            // Native multipath: hidden path devices nvme<inst>c<ctrl>n<nsid>.
            let mut paths: Vec<(String, String, Option<String>)> = all_block
                .iter()
                .filter_map(|b| match split_dev(b) {
                    Some((i, Some(c), n)) if i == inst && n == nsid => {
                        Some((b.clone(), c, read(&block.join(b).join("ana_state"))))
                    }
                    _ => None,
                })
                .collect();
            if paths.is_empty() {
                // No native multipath: the namespace sits on one controller
                // (nvme<ctrl>n<nsid>, instance = controller number).
                let c = format!("nvme{inst}");
                if ctrls.contains(c.as_str()) || ctrls.is_empty() {
                    paths.push((e.clone(), c, read(&block.join(e).join("ana_state"))));
                }
            }
            namespaces.push(Namespace {
                dev: e.clone(),
                nsid: Some(nsid),
                wwid,
                paths,
            });
        }
        out.push(Subsystem {
            subsysnqn: read(&d.join("subsysnqn")).unwrap_or_default(),
            model: read(&d.join("model")),
            iopolicy: read(&d.join("iopolicy")),
            name: s,
            namespaces,
        });
    }
    out
}

/// Is native NVMe multipath on (`/sys/module/nvme_core/parameters/multipath`)?
pub fn native_multipath(root: &Path) -> Option<bool> {
    read(&root.join("sys/module/nvme_core/parameters/multipath")).map(|v| v == "Y" || v == "1")
}

fn path_state(ctrl: Option<&Controller>, ana: Option<&str>) -> PathState {
    match (ctrl, ana) {
        (Some(c), _) if !c.is_live() => PathState::Failed,
        (None, _) => PathState::Failed,
        (_, Some("optimized")) | (_, None) => PathState::Active,
        (_, Some("non-optimized")) => PathState::Standby,
        (_, Some(_)) => PathState::Failed,
    }
}

/// The path view of the NVMe-oF namespaces of `nqns` (subsystems this node
/// connects to). Namespaces listed in `in_dm_maps` (native multipath off,
/// dm-multipath owns them) are left to the dm view.
pub fn assemble(
    subsystems: &[Subsystem],
    controllers: &[Controller],
    nqns: &HashSet<String>,
    in_dm_maps: &HashSet<String>,
    size_of: impl Fn(&str) -> Option<u64>,
) -> Vec<LunView> {
    let mut out = Vec::new();
    for s in subsystems.iter().filter(|s| nqns.contains(&s.subsysnqn)) {
        // Without native multipath every path is a separate subsystem
        // instance; group the namespaces by WWID across them.
        for ns in &s.namespaces {
            if in_dm_maps.contains(&ns.dev) {
                continue;
            }
            let mut paths: Vec<PathView> = ns
                .paths
                .iter()
                .map(|(dev, c, ana)| {
                    let ctrl = controllers.iter().find(|x| &x.name == c);
                    PathView {
                        dev: dev.clone(),
                        transport: "nvme".into(),
                        target: Some(s.subsysnqn.clone()),
                        portal: ctrl.and_then(Controller::address),
                        local_address: ctrl.and_then(|c| c.local_address.clone()),
                        lun: ns.nsid,
                        state: path_state(ctrl, ana.as_deref()),
                        dm_state: None,
                        checker_state: ana.clone(),
                        device_state: ctrl.map(|c| c.state.clone()),
                        session_state: ctrl.map(|c| format!("{} {}", c.name, c.transport)),
                    }
                })
                .collect();
            paths.sort_by(|a, b| a.dev.cmp(&b.dev));
            let wwid = ns
                .wwid
                .clone()
                .unwrap_or_else(|| format!("{}:ns{}", s.subsysnqn, ns.nsid.unwrap_or(0)));
            if let Some(l) = out
                .iter_mut()
                .find(|l: &&mut LunView| l.wwid == wwid && l.map.is_none())
            {
                l.paths.extend(paths);
                l.paths.sort_by(|a, b| a.dev.cmp(&b.dev));
                super::paths::recount(l);
                continue;
            }
            let native = ns.paths.iter().any(|(d, _, _)| d != &ns.dev);
            let mut l = LunView {
                wwid,
                // The native multipath head plays the role of the map.
                map: native.then(|| ns.dev.clone()),
                device: format!("/dev/{}", ns.dev),
                size_bytes: size_of(&ns.dev),
                vendor: None,
                product: s.model.clone(),
                target: Some(s.subsysnqn.clone()),
                lun: ns.nsid,
                paths,
                active_paths: 0,
                failed_paths: 0,
            };
            super::paths::recount(&mut l);
            out.push(l);
        }
    }
    out
}

// ------------------------------------------------------------ host tools

async fn run(prog: &str, args: &[&str], t: Duration) -> Result<Run, String> {
    super::iscsi::run(prog, args, t).await
}

/// Make sure the fabrics transport module is loaded; `Err` = the kernel
/// has no such transport (a 424 for the API).
pub async fn load_transport(tr: NvmeTransport, t: Duration) -> Result<(), String> {
    let m = tr.module();
    if Path::new("/sys/module").join(m).exists() {
        return Ok(());
    }
    let _ = run("modprobe", &[m], t).await;
    if Path::new("/sys/module").join(m).exists() {
        Ok(())
    } else {
        Err(format!("kernel module {m} is not available"))
    }
}

/// Give the host a stable NQN (`/etc/nvme/hostnqn`) when it has none, so
/// arrays see the same initiator after every reboot. Never overwrites one.
pub async fn ensure_hostnqn(t: Duration) -> Result<Option<String>, String> {
    let p = PathBuf::from(HOSTNQN_PATH);
    if read(&p).is_some_and(|s| valid_nqn(&s)) {
        return Ok(None);
    }
    let r = run("nvme", &["gen-hostnqn"], t).await?;
    let nqn = r.stdout.trim().to_string();
    if !r.ok || !valid_nqn(&nqn) {
        return Err(format!("nvme gen-hostnqn: {}", r.stderr.trim()));
    }
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    std::fs::write(&p, format!("{nqn}\n")).map_err(|e| format!("{HOSTNQN_PATH}: {e}"))?;
    Ok(Some(nqn))
}

/// Discovery through every listed address; one that does not answer is
/// reported in the error list but does not stop the others.
pub async fn discover(
    transport: NvmeTransport,
    addresses: &[Portal],
    t: Duration,
) -> (Vec<DiscoveredSubsystem>, Vec<String>) {
    let mut recs = Vec::new();
    let mut errs = Vec::new();
    for a in addresses {
        let port = a.port.to_string();
        let r = run(
            "nvme",
            &[
                "discover",
                "-t",
                transport.as_str(),
                "-a",
                &a.host,
                "-s",
                &port,
                "-o",
                "json",
            ],
            t,
        )
        .await;
        match r {
            Ok(r) if r.ok => match parse_discover_json(&r.stdout) {
                Ok(v) => recs.extend(v),
                Err(e) => errs.push(format!("{a}: {e}")),
            },
            Ok(r) => errs.push(format!("{a}: {}", r.stderr.trim())),
            Err(e) => errs.push(format!("{a}: {e}")),
        }
    }
    (group_discovery(&recs), errs)
}

/// The `nvme connect` arguments for one path.
pub fn connect_args(spec: &NvmeTargetSpec, a: &Portal) -> Vec<String> {
    vec![
        "connect".into(),
        "-t".into(),
        spec.transport.as_str().into(),
        "-a".into(),
        a.host.clone(),
        "-s".into(),
        a.port.to_string(),
        "-n".into(),
        spec.subnqn.clone(),
        format!("--ctrl-loss-tmo={}", spec.ctrl_loss_tmo_secs),
        format!("--reconnect-delay={}", spec.reconnect_delay_secs),
    ]
}

/// Is `a` already a controller of `nqn` on this host?
pub fn connected(ctrls: &[Controller], nqn: &str, a: &Portal) -> bool {
    let want = a.to_string();
    ctrls.iter().any(|c| {
        c.subsysnqn == nqn && c.state != "deleting" && c.address().as_deref() == Some(&want)
    })
}

/// Connect one path (an existing controller is fine).
pub async fn ensure_path(spec: &NvmeTargetSpec, a: &Portal, t: Duration) -> Result<bool, String> {
    if connected(&scan_controllers(Path::new("/")), &spec.subnqn, a) {
        return Ok(false);
    }
    let args = connect_args(spec, a);
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let r = run("nvme", &argv, t).await?;
    if r.ok || r.stderr.contains("already connected") {
        Ok(r.ok)
    } else {
        Err(format!("{a}: connect: {}", r.stderr.trim()))
    }
}

/// Disconnect one controller (`nvme disconnect -d nvmeN`).
pub async fn disconnect(ctrl: &str, t: Duration) -> Result<(), String> {
    if !ctrl.starts_with("nvme") || !ctrl[4..].chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("bad controller {ctrl:?}"));
    }
    let r = run("nvme", &["disconnect", "-d", ctrl], t).await?;
    if r.ok {
        Ok(())
    } else {
        Err(format!("disconnect {ctrl}: {}", r.stderr.trim()))
    }
}

/// Set a subsystem's native multipath I/O policy; true if it changed.
pub fn set_iopolicy(root: &Path, subsys: &str, p: NvmeIoPolicy) -> Result<bool, String> {
    let f = root
        .join("sys/class/nvme-subsystem")
        .join(subsys)
        .join("iopolicy");
    match read(&f) {
        Some(cur) if cur == p.as_sysfs() => Ok(false),
        Some(_) => std::fs::write(&f, p.as_sysfs())
            .map(|_| true)
            .map_err(|e| format!("{}: {e}", f.display())),
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NQN: &str = "nqn.2014-08.org.example:hxsan";

    fn spec() -> NvmeTargetSpec {
        serde_json::from_value(serde_json::json!({
            "id": "n1", "subnqn": NQN, "addresses": ["10.0.1.1", "10.0.2.1:4420"]
        }))
        .unwrap()
    }

    #[test]
    fn nqn_validation() {
        assert!(valid_nqn(NQN));
        assert!(valid_nqn(
            "nqn.2014-08.org.nvmexpress:uuid:8f2c0ab1-1fbb-4a4f-9d4c-0e5b1a2b3c4d"
        ));
        for bad in [
            "",
            "iqn.2003-01.org.x:t",
            "nqn.14-08.org",
            "nqn.2014-08.",
            "nqn.2014-08.org x",
        ] {
            assert!(!valid_nqn(bad), "{bad}");
        }
        assert!(!valid_nqn(&format!("nqn.2014-08.org.{}", "a".repeat(220))));
    }

    #[test]
    fn spec_defaults_and_validation() {
        let mut s = spec();
        assert_eq!(s.transport, NvmeTransport::Tcp);
        assert!(s.use_advertised_addresses);
        assert_eq!(s.ctrl_loss_tmo_secs, 600);
        assert_eq!(s.iopolicy, NvmeIoPolicy::RoundRobin);
        s.validate().unwrap();
        // discovery addresses default to 8009
        assert_eq!(s.addresses, vec!["10.0.1.1:8009", "10.0.2.1:4420"]);
        let mut s = spec();
        s.use_advertised_addresses = false;
        s.validate().unwrap();
        assert_eq!(s.addresses, vec!["10.0.1.1:4420", "10.0.2.1:4420"]);
        let mut s = spec();
        s.subnqn = DISCOVERY_NQN.into();
        assert!(s.validate().is_err());
        let mut s = spec();
        s.ctrl_loss_tmo_secs = -2;
        assert!(s.validate().is_err());
        let mut s = spec();
        s.addresses.clear();
        assert!(s.validate().is_err());
        assert_eq!(
            serde_json::to_value(NvmeIoPolicy::QueueDepth).unwrap(),
            "queue-depth"
        );
    }

    const DISCOVER: &str = r#"{
      "device":"nvme2","genctr":4,
      "records":[
        {"trtype":"tcp","adrfam":"ipv4","subtype":"current discovery subsystem","treq":"not specified","portid":1,"trsvcid":"4420","subnqn":"nqn.2014-08.org.nvmexpress.discovery","traddr":"10.0.1.1"},
        {"trtype":"tcp","adrfam":"ipv4","subtype":"nvme subsystem","treq":"not specified","portid":1,"trsvcid":"4420","subnqn":"nqn.2014-08.org.example:hxsan","traddr":"10.0.1.1"},
        {"trtype":"tcp","adrfam":"ipv4","subtype":"nvme subsystem","treq":"not specified","portid":2,"trsvcid":"4420","subnqn":"nqn.2014-08.org.example:hxsan","traddr":"10.0.2.1"},
        {"trtype":"tcp","adrfam":"ipv6","subtype":"nvme subsystem","portid":3,"trsvcid":4421,"subnqn":"nqn.2014-08.org.example:other","traddr":"fd00::1"}
      ]}"#;

    #[test]
    fn discovery_collects_every_path() {
        let r = parse_discover_json(DISCOVER).unwrap();
        assert_eq!(r.len(), 4);
        let g = group_discovery(&r);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].subnqn, "nqn.2014-08.org.example:hxsan");
        assert_eq!(g[0].addresses, vec!["10.0.1.1:4420", "10.0.2.1:4420"]);
        assert_eq!(g[1].addresses, vec!["[fd00::1]:4421"]);
        // the same records from a second discovery controller merge
        let mut twice = r.clone();
        twice.extend(r);
        assert_eq!(group_discovery(&twice), g);
        let mut s = spec();
        s.validate().unwrap();
        let c: Vec<String> = connect_addresses(&s, &g)
            .iter()
            .map(|p| p.to_string())
            .collect();
        assert_eq!(c, vec!["10.0.1.1:4420", "10.0.2.1:4420"]);
        s.use_advertised_addresses = false;
        s.addresses = vec!["10.0.9.9".into()];
        s.validate().unwrap();
        let c: Vec<String> = connect_addresses(&s, &g)
            .iter()
            .map(|p| p.to_string())
            .collect();
        assert_eq!(c, vec!["10.0.9.9:4420"]);
        assert!(parse_discover_json("Failed to write to /dev/nvme-fabrics").is_err());
        assert!(parse_discover_json("[]").unwrap().is_empty());
    }

    #[test]
    fn connect_command() {
        let mut s = spec();
        s.validate().unwrap();
        let a = Portal::parse_default("10.0.2.1", DEFAULT_IO_PORT).unwrap();
        assert_eq!(
            connect_args(&s, &a).join(" "),
            "connect -t tcp -a 10.0.2.1 -s 4420 -n nqn.2014-08.org.example:hxsan --ctrl-loss-tmo=600 --reconnect-delay=10"
        );
    }

    #[test]
    fn dev_names() {
        assert_eq!(split_dev("nvme0c1n1"), Some((0, Some("nvme1".into()), 1)));
        assert_eq!(split_dev("nvme3n12"), Some((3, None, 12)));
        assert_eq!(split_dev("nvme0"), None);
        assert_eq!(split_dev("nvme-subsys0"), None);
        assert_eq!(split_dev("sda"), None);
        assert_eq!(
            parse_ctrl_address("traddr=10.0.1.1,trsvcid=4420,src_addr=10.0.1.20\n"),
            (
                Some("10.0.1.1".into()),
                Some("4420".into()),
                Some("10.0.1.20".into())
            )
        );
        assert_eq!(
            parse_ctrl_address("traddr=10.0.1.1,trsvcid=4420,host_traddr=10.0.1.21").2,
            Some("10.0.1.21".into())
        );
    }

    fn w(root: &Path, rel: &str, v: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, v).unwrap();
    }

    /// A local PCIe drive (nvme0, subsys0) and a fabrics subsystem with two
    /// TCP controllers (nvme1, nvme2) under native multipath (subsys1).
    fn tree(dead_second: bool) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        w(r, "sys/module/nvme_core/parameters/multipath", "Y\n");
        w(
            r,
            "sys/class/nvme/nvme0/subsysnqn",
            "nqn.2014.08.org.nvmexpress:local\n",
        );
        w(r, "sys/class/nvme/nvme0/transport", "pcie\n");
        w(r, "sys/class/nvme/nvme0/state", "live\n");
        w(r, "sys/class/nvme/nvme0/address", "0000:01:00.0\n");
        for (c, a, local, st) in [
            ("nvme1", "10.0.1.1", "10.0.1.20", "live"),
            (
                "nvme2",
                "10.0.2.1",
                "10.0.2.20",
                if dead_second { "connecting" } else { "live" },
            ),
        ] {
            w(
                r,
                &format!("sys/class/nvme/{c}/subsysnqn"),
                &format!("{NQN}\n"),
            );
            w(r, &format!("sys/class/nvme/{c}/transport"), "tcp\n");
            w(r, &format!("sys/class/nvme/{c}/state"), &format!("{st}\n"));
            w(
                r,
                &format!("sys/class/nvme/{c}/address"),
                &format!("traddr={a},trsvcid=4420,src_addr={local}\n"),
            );
        }
        w(
            r,
            "sys/class/nvme-subsystem/nvme-subsys0/subsysnqn",
            "nqn.2014.08.org.nvmexpress:local\n",
        );
        w(r, "sys/class/nvme-subsystem/nvme-subsys0/nvme0/x", "");
        w(
            r,
            "sys/class/nvme-subsystem/nvme-subsys0/nvme0n1/wwid",
            "eui.local\n",
        );
        w(r, "sys/block/nvme0n1/size", "100\n");
        w(
            r,
            "sys/class/nvme-subsystem/nvme-subsys1/subsysnqn",
            &format!("{NQN}\n"),
        );
        w(r, "sys/class/nvme-subsystem/nvme-subsys1/model", "Linux\n");
        w(
            r,
            "sys/class/nvme-subsystem/nvme-subsys1/iopolicy",
            "numa\n",
        );
        w(r, "sys/class/nvme-subsystem/nvme-subsys1/nvme1/x", "");
        w(r, "sys/class/nvme-subsystem/nvme-subsys1/nvme2/x", "");
        w(
            r,
            "sys/class/nvme-subsystem/nvme-subsys1/nvme1n1/wwid",
            "uuid.6b1a\n",
        );
        w(r, "sys/block/nvme1n1/size", "2048\n");
        w(r, "sys/block/nvme1c1n1/ana_state", "optimized\n");
        w(r, "sys/block/nvme1c2n1/ana_state", "optimized\n");
        d
    }

    #[test]
    fn sysfs_native_multipath_view() {
        let d = tree(false);
        let r = d.path();
        assert_eq!(native_multipath(r), Some(true));
        let c = scan_controllers(r);
        assert_eq!(c.len(), 3);
        assert_eq!(c[1].address().as_deref(), Some("10.0.1.1:4420"));
        assert_eq!(c[1].local_address.as_deref(), Some("10.0.1.20"));
        let s = scan_subsystems(r);
        assert_eq!(s.len(), 2);
        assert_eq!(s[1].namespaces.len(), 1);
        assert_eq!(s[1].namespaces[0].paths.len(), 2);
        let nqns: HashSet<String> = [NQN.to_string()].into_iter().collect();
        let v = assemble(&s, &c, &nqns, &HashSet::new(), |_| Some(1 << 20));
        assert_eq!(v.len(), 1, "only our subsystem, not the local drive: {v:?}");
        let l = &v[0];
        assert_eq!(l.wwid, "uuid.6b1a");
        assert_eq!(l.map.as_deref(), Some("nvme1n1"));
        assert_eq!(l.device, "/dev/nvme1n1");
        assert_eq!(l.target.as_deref(), Some(NQN));
        assert_eq!((l.active_paths, l.failed_paths), (2, 0));
        assert_eq!(l.paths[0].dev, "nvme1c1n1");
        assert_eq!(l.paths[0].portal.as_deref(), Some("10.0.1.1:4420"));
        assert_eq!(l.paths[1].portal.as_deref(), Some("10.0.2.1:4420"));
        assert_eq!(l.paths[0].transport, "nvme");
        assert!(connected(
            &c,
            NQN,
            &Portal::parse_default("10.0.2.1", 4420).unwrap()
        ));
        assert!(!connected(
            &c,
            NQN,
            &Portal::parse_default("10.0.3.1", 4420).unwrap()
        ));
        // I/O policy
        assert!(set_iopolicy(r, "nvme-subsys1", NvmeIoPolicy::RoundRobin).unwrap());
        assert!(!set_iopolicy(r, "nvme-subsys1", NvmeIoPolicy::RoundRobin).unwrap());
        assert!(!set_iopolicy(r, "nvme-subsys9", NvmeIoPolicy::RoundRobin).unwrap());
    }

    #[test]
    fn a_lost_controller_is_a_failed_path() {
        let d = tree(true);
        let r = d.path();
        let nqns: HashSet<String> = [NQN.to_string()].into_iter().collect();
        let v = assemble(
            &scan_subsystems(r),
            &scan_controllers(r),
            &nqns,
            &HashSet::new(),
            |_| None,
        );
        assert_eq!((v[0].active_paths, v[0].failed_paths), (1, 1));
        assert_eq!(v[0].paths[1].state, PathState::Failed);
        assert_eq!(v[0].paths[1].device_state.as_deref(), Some("connecting"));
    }

    #[test]
    fn without_native_multipath_namespaces_group_by_wwid() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        w(r, "sys/module/nvme_core/parameters/multipath", "N\n");
        for (i, a) in [(1, "10.0.1.1"), (2, "10.0.2.1")] {
            w(r, &format!("sys/class/nvme/nvme{i}/subsysnqn"), NQN);
            w(r, &format!("sys/class/nvme/nvme{i}/transport"), "tcp");
            w(r, &format!("sys/class/nvme/nvme{i}/state"), "live");
            w(
                r,
                &format!("sys/class/nvme/nvme{i}/address"),
                &format!("traddr={a},trsvcid=4420"),
            );
            w(
                r,
                &format!("sys/class/nvme-subsystem/nvme-subsys{i}/subsysnqn"),
                NQN,
            );
            w(
                r,
                &format!("sys/class/nvme-subsystem/nvme-subsys{i}/nvme{i}/x"),
                "",
            );
            w(
                r,
                &format!("sys/class/nvme-subsystem/nvme-subsys{i}/nvme{i}n1/wwid"),
                "uuid.6b1a",
            );
            w(r, &format!("sys/block/nvme{i}n1/size"), "8");
        }
        let nqns: HashSet<String> = [NQN.to_string()].into_iter().collect();
        let s = scan_subsystems(r);
        let c = scan_controllers(r);
        let v = assemble(&s, &c, &nqns, &HashSet::new(), |_| None);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].map, None);
        assert_eq!(v[0].paths.len(), 2);
        assert_eq!(v[0].active_paths, 2);
        // once dm-multipath maps them, the dm view owns them
        let dm: HashSet<String> = ["nvme1n1".to_string(), "nvme2n1".to_string()]
            .into_iter()
            .collect();
        assert!(assemble(&s, &c, &nqns, &dm, |_| None).is_empty());
    }
}
