//! Declarative cluster configuration: a spec file describes the desired
//! state, `plan` diffs it against the live node, `apply` makes it so and
//! `export` dumps the live state as a spec. See
//! `docs/design/declarative-config.md`.
//!
//! This file is pure (no I/O, no init system, no kernel): spec types,
//! validation, diff and export. `exec.rs` talks to the managers; the same
//! code therefore behaves identically on OpenRC and systemd nodes.
pub mod exec;
pub mod kinds;

use crate::api::sdn_zones::{
    parse_cidr, subnet_id, validate_subnet, validate_vnet, validate_zone, Subnet, VNet, Zone,
};
use horcrux_common::{
    VmArchitecture, VmConfig, VmDisk, VmDisplay, VmHardware, VmHypervisor, VmNic, VmStatus,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const SPEC_VERSION: u32 = 1;

fn one() -> u32 {
    SPEC_VERSION
}
fn yes() -> bool {
    true
}
fn default_cpus() -> u32 {
    1
}
fn default_bus() -> String {
    "virtio".into()
}

/// The desired state. A section that is absent is *unmanaged* (never
/// pruned); a present, empty one means "there should be none".
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Spec {
    #[serde(default = "one")]
    pub version: u32,
    /// Delete live resources of managed sections that the spec lacks
    /// (the request can override it).
    #[serde(default)]
    pub prune: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_pools: Option<Vec<PoolSpec>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub networks: Option<NetworksSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vms: Option<Vec<VmSpec>>,
    /// Sections of registered kinds (`kinds::registry()`); anything else is
    /// rejected with a clear message.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct NetworksSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zones: Option<Vec<Zone>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vnets: Option<Vec<VNet>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subnets: Option<Vec<Subnet>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PoolSpec {
    pub name: String,
    #[serde(rename = "type", alias = "storage_type")]
    pub storage_type: String,
    pub path: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Power {
    Running,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiskSpec {
    pub size_gb: u64,
    #[serde(default = "default_bus")]
    pub bus: String,
    #[serde(default)]
    pub cache: String,
    /// Storage pool NAME (resolved to the pool id at apply time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub discard: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ssd: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub iothread: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VmSpec {
    /// Identity in the spec (VMs are matched by name).
    pub name: String,
    /// VM id; defaults to the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default = "default_hv")]
    pub hypervisor: VmHypervisor,
    /// MB
    pub memory: u64,
    #[serde(default = "default_cpus")]
    pub cpus: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub architecture: Option<VmArchitecture>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disks: Vec<DiskSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<VmDisplay>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardware: Option<VmHardware>,
    /// LXD/Incus VMs: the instance image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Desired power state; absent = leave it alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<Power>,
}

fn default_hv() -> VmHypervisor {
    VmHypervisor::Qemu
}

impl VmSpec {
    pub fn vm_id(&self) -> String {
        self.id.clone().unwrap_or_else(|| self.name.clone())
    }
}

// ---------------------------------------------------------------- parsing

/// Parse YAML (or JSON, which is YAML) and validate the spec on its own.
pub fn parse(text: &str) -> Result<Spec, String> {
    let mut spec: Spec = serde_yaml::from_str(text).map_err(|e| format!("spec: {e}"))?;
    normalize(&mut spec)?;
    validate(&spec)?;
    Ok(spec)
}

/// Derived fields: a subnet's id comes from its VNet and CIDR.
fn normalize(spec: &mut Spec) -> Result<(), String> {
    if let Some(n) = spec.networks.as_mut() {
        for s in n.subnets.iter_mut().flatten() {
            let (net, p) = parse_cidr(&s.cidr).map_err(|e| format!("subnet {}: {e}", s.cidr))?;
            s.id = subnet_id(&s.vnet, net, p);
            s.gateway = s.gateway.take().filter(|g| !g.is_empty());
        }
    }
    Ok(())
}

fn dup<'a>(kind: &str, names: impl Iterator<Item = &'a str>) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for n in names {
        if !seen.insert(n) {
            return Err(format!("{kind} {n:?} is listed twice"));
        }
    }
    Ok(())
}

/// Checks that need nothing but the spec itself.
pub fn validate(spec: &Spec) -> Result<(), String> {
    if spec.version != SPEC_VERSION {
        return Err(format!(
            "spec version {} is not supported (this server reads version {SPEC_VERSION})",
            spec.version
        ));
    }
    for (k, section) in &spec.extra {
        match kinds::find(k) {
            Some(kind) => kind.validate(section).map_err(|e| format!("{k}: {e}"))?,
            None => {
                let mut known = vec!["storage_pools", "networks", "vms"];
                known.extend(kinds::keys());
                return Err(format!(
                    "section {k:?} is not supported yet (this server manages {})",
                    known.join(", ")
                ));
            }
        }
    }
    if let Some(pools) = &spec.storage_pools {
        dup("storage pool", pools.iter().map(|p| p.name.as_str()))?;
        for p in pools {
            if p.name.trim().is_empty() || p.path.trim().is_empty() {
                return Err("storage pool: name and path are required".into());
            }
            pool_type(&p.storage_type)?;
        }
    }
    if let Some(n) = &spec.networks {
        let zones = n.zones.as_deref().unwrap_or(&[]);
        dup("zone", zones.iter().map(|z| z.id.as_str()))?;
        for z in zones {
            validate_zone(z).map_err(|e| format!("zone {}: {e}", z.id))?;
        }
        let vnets = n.vnets.as_deref().unwrap_or(&[]);
        dup("vnet", vnets.iter().map(|v| v.id.as_str()))?;
        for v in vnets {
            // The zone may be live rather than in the spec: type-specific
            // checks run in `plan` once both are known.
            if !crate::api::sdn_zones::valid_vnet_id(&v.id) {
                return Err(format!(
                    "vnet {}: 1-8 chars, lower-case letters and digits, starting with a letter",
                    v.id
                ));
            }
        }
        let subs = n.subnets.as_deref().unwrap_or(&[]);
        dup("subnet", subs.iter().map(|s| s.id.as_str()))?;
        for s in subs {
            validate_subnet(s, &[]).map_err(|e| format!("subnet {}: {e}", s.cidr))?;
        }
        for (i, a) in subs.iter().enumerate() {
            if let Some(b) = subs[i + 1..]
                .iter()
                .find(|b| crate::api::sdn_zones::subnets_overlap(&a.cidr, &b.cidr))
            {
                return Err(format!("subnets {} and {} overlap", a.cidr, b.cidr));
            }
        }
    }
    if let Some(vms) = &spec.vms {
        dup("vm", vms.iter().map(|v| v.name.as_str()))?;
        dup(
            "vm id",
            vms.iter()
                .map(|v| v.vm_id())
                .collect::<Vec<_>>()
                .iter()
                .map(|s| s.as_str()),
        )?;
        for v in vms {
            horcrux_core::validation::validate_vm_id(&v.vm_id())
                .map_err(|e| format!("vm {}: {e:?}", v.name))?;
            if v.memory == 0 || v.cpus == 0 {
                return Err(format!("vm {}: memory and cpus must be > 0", v.name));
            }
            for (i, d) in v.disks.iter().enumerate() {
                to_disk(d, None)
                    .validate(i)
                    .map_err(|e| format!("vm {}: {e}", v.name))?;
            }
        }
    }
    Ok(())
}

pub fn pool_type(s: &str) -> Result<crate::storage::StorageType, String> {
    use crate::storage::StorageType as T;
    Ok(match s.to_lowercase().as_str() {
        "zfs" => T::Zfs,
        "ceph" => T::Ceph,
        "lvm" => T::Lvm,
        "iscsi" => T::Iscsi,
        "directory" | "dir" => T::Directory,
        "cifs" => T::Cifs,
        "nfs" => T::Nfs,
        "glusterfs" => T::GlusterFs,
        "cephfs" => T::CephFs,
        "btrfs" => T::BtrFs,
        "s3" => T::S3,
        _ => return Err(format!("unknown storage type {s:?}")),
    })
}

pub fn pool_type_name(t: &crate::storage::StorageType) -> String {
    serde_json::to_value(t)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

pub fn to_disk(d: &DiskSpec, pool_id: Option<String>) -> VmDisk {
    VmDisk {
        path: String::new(),
        size_gb: d.size_gb,
        disk_type: d.bus.clone(),
        cache: d.cache.clone(),
        discard: d.discard,
        ssd: d.ssd,
        iothread: d.iothread,
        storage: pool_id,
    }
}

// ------------------------------------------------------------------ plan

/// What the node has now.
#[derive(Debug, Clone, Default)]
pub struct Live {
    pub pools: Vec<crate::storage::StoragePool>,
    pub zones: Vec<Zone>,
    pub vnets: Vec<VNet>,
    pub subnets: Vec<Subnet>,
    pub vms: Vec<VmConfig>,
    /// Live state of the registered kinds (`kinds::registry()`), keyed by
    /// spec section and shaped like that section.
    pub extra: BTreeMap<String, serde_json::Value>,
    /// Kinds whose `gather` failed (section -> message).
    pub extra_errors: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Create,
    Update,
    Delete,
    Start,
    Stop,
    /// Differs, but cannot be changed in place (details say why).
    Blocked,
}

/// One differing field: what the spec wants and what the node has.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FieldDiff {
    pub field: String,
    pub expected: String,
    pub actual: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Change {
    /// storage_pool | zone | vnet | subnet | vm | (a registered kind's)
    pub kind: String,
    pub name: String,
    pub action: Action,
    pub details: Vec<String>,
    /// Per-field expected/actual (drift view); empty for create/delete.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<FieldDiff>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Plan {
    pub prune: bool,
    pub changes: Vec<Change>,
    /// The spec cannot be applied at all while these stand.
    pub errors: Vec<String>,
}

impl Plan {
    /// Nothing to do and nothing wrong.
    pub fn in_sync(&self) -> bool {
        self.changes.is_empty() && self.errors.is_empty()
    }
    /// Changes that the engine cannot carry out in place.
    pub fn blocked(&self) -> usize {
        self.changes
            .iter()
            .filter(|c| c.action == Action::Blocked)
            .count()
    }
}

/// A change without per-field detail (create, delete, power).
pub fn ch(kind: &str, name: &str, action: Action, details: Vec<String>) -> Change {
    Change {
        kind: kind.into(),
        name: name.into(),
        action,
        details,
        fields: vec![],
    }
}

/// Field differences collected while comparing one resource. `field` records
/// a structured difference (and its text line); `note` adds explanation.
#[derive(Debug, Default)]
pub struct Diffs {
    pub lines: Vec<String>,
    pub fields: Vec<FieldDiff>,
}

fn show<T: Serialize + ?Sized>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => s,
        Ok(serde_json::Value::Null) => "none".into(),
        Ok(v) => v.to_string(),
        Err(_) => "?".into(),
    }
}

impl Diffs {
    /// Record `what` if the live value differs from the wanted one.
    pub fn field<T: PartialEq + Serialize>(&mut self, what: &str, live: &T, want: &T) {
        if live != want {
            self.push(what, show(want), show(live));
        }
    }
    /// Record a difference whose values are already rendered.
    pub fn push(&mut self, what: &str, expected: String, actual: String) {
        self.lines.push(format!("{what}: {actual} -> {expected}"));
        self.fields.push(FieldDiff {
            field: what.into(),
            expected,
            actual,
        });
    }
    pub fn note(&mut self, s: impl Into<String>) {
        self.lines.push(s.into());
    }
    /// No field differs (notes don't count).
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
    pub fn into_change(self, kind: &str, name: &str, action: Action) -> Change {
        Change {
            kind: kind.into(),
            name: name.into(),
            action,
            details: self.lines,
            fields: self.fields,
        }
    }
}

/// Dependency rank of the built-in kinds (see `kinds` for the rest).
fn rank(kind: &str) -> u32 {
    match kind {
        "storage_pool" => 10,
        "zone" => 20,
        "vnet" => 30,
        "subnet" => 40,
        "vm" => 50,
        k => kinds::owner_of(k).map(|o| o.rank()).unwrap_or(900),
    }
}

pub fn plan(spec: &Spec, live: &Live, prune: bool) -> Plan {
    let mut p = Plan {
        prune,
        ..Default::default()
    };
    let mut creates: Vec<Change> = Vec::new();
    let mut deletes: Vec<Change> = Vec::new();

    // storage pools, by name
    let want_pool_names: BTreeSet<&str> = spec
        .storage_pools
        .iter()
        .flatten()
        .map(|x| x.name.as_str())
        .collect();
    for w in spec.storage_pools.iter().flatten() {
        match live.pools.iter().find(|l| l.name == w.name) {
            None => creates.push(ch(
                "storage_pool",
                &w.name,
                Action::Create,
                vec![format!("{} at {}", w.storage_type, w.path)],
            )),
            Some(l) => {
                let want_type =
                    pool_type_name(&pool_type(&w.storage_type).unwrap_or(l.storage_type.clone()));
                let mut fixed = Diffs::default();
                fixed.field("type", &pool_type_name(&l.storage_type), &want_type);
                fixed.field("path", &l.path, &w.path);
                if !fixed.is_empty() {
                    fixed.note(
                        "a pool's type and path can't be edited in place: remove and re-add it",
                    );
                    let mut d = fixed;
                    d.field("enabled", &l.enabled, &w.enabled);
                    creates.push(d.into_change("storage_pool", &w.name, Action::Blocked));
                } else {
                    let mut d = Diffs::default();
                    d.field("enabled", &l.enabled, &w.enabled);
                    if !d.is_empty() {
                        creates.push(d.into_change("storage_pool", &w.name, Action::Update));
                    }
                }
            }
        }
    }
    if prune && spec.storage_pools.is_some() {
        for l in live
            .pools
            .iter()
            .filter(|l| !want_pool_names.contains(l.name.as_str()))
        {
            deletes.push(ch("storage_pool", &l.name, Action::Delete, vec![]));
        }
    }

    // networks
    if let Some(n) = &spec.networks {
        let want_zones = n.zones.as_deref();
        for w in want_zones.into_iter().flatten() {
            match live.zones.iter().find(|l| l.id == w.id) {
                None => creates.push(ch("zone", &w.id, Action::Create, vec![w.zone_type.clone()])),
                Some(l) if l != w => {
                    let mut d = Diffs::default();
                    d.field("type", &l.zone_type, &w.zone_type);
                    d.field("description", &l.description, &w.description);
                    d.field("uplink", &l.uplink, &w.uplink);
                    d.field("local_ip", &l.local_ip, &w.local_ip);
                    d.field("peers", &l.peers, &w.peers);
                    d.field("mtu", &l.mtu, &w.mtu);
                    d.field("dst_port", &l.dst_port, &w.dst_port);
                    let blocked =
                        l.zone_type != w.zone_type && live.vnets.iter().any(|v| v.zone == w.id);
                    if blocked {
                        d.note("the zone type can't change while VNets use the zone");
                    }
                    creates.push(d.into_change(
                        "zone",
                        &w.id,
                        if blocked {
                            Action::Blocked
                        } else {
                            Action::Update
                        },
                    ));
                }
                Some(_) => {}
            }
        }
        for w in n.vnets.iter().flatten() {
            match live.vnets.iter().find(|l| l.id == w.id) {
                None => {
                    let zone = want_zones
                        .into_iter()
                        .flatten()
                        .chain(live.zones.iter())
                        .find(|z| z.id == w.zone);
                    match zone {
                        None => p
                            .errors
                            .push(format!("vnet {}: zone {} does not exist", w.id, w.zone)),
                        Some(z) => {
                            if let Err(e) = validate_vnet(w, z) {
                                p.errors.push(format!("vnet {}: {e}", w.id));
                            }
                        }
                    }
                    creates.push(ch(
                        "vnet",
                        &w.id,
                        Action::Create,
                        vec![format!("zone {}", w.zone)],
                    ));
                }
                Some(l) if l != w => {
                    let mut d = Diffs::default();
                    d.field("zone", &l.zone, &w.zone);
                    let moved = !d.is_empty();
                    d.field("alias", &l.alias, &w.alias);
                    d.field("tag", &l.tag, &w.tag);
                    d.field("vlan_aware", &l.vlan_aware, &w.vlan_aware);
                    if moved {
                        d.note("a VNet can't move to another zone: delete and re-create it");
                        creates.push(d.into_change("vnet", &w.id, Action::Blocked));
                    } else {
                        creates.push(d.into_change("vnet", &w.id, Action::Update));
                    }
                }
                Some(_) => {}
            }
        }
        for w in n.subnets.iter().flatten() {
            let vnet_known = n.vnets.iter().flatten().any(|v| v.id == w.vnet)
                || live.vnets.iter().any(|v| v.id == w.vnet);
            if !vnet_known {
                p.errors
                    .push(format!("subnet {}: vnet {} does not exist", w.cidr, w.vnet));
            }
            match live.subnets.iter().find(|l| l.id == w.id) {
                None => creates.push(ch("subnet", &w.id, Action::Create, vec![w.cidr.clone()])),
                Some(l) if l != w => {
                    let mut d = Diffs::default();
                    d.field("gateway", &l.gateway, &w.gateway);
                    d.field("snat", &l.snat, &w.snat);
                    creates.push(d.into_change("subnet", &w.id, Action::Update));
                }
                Some(_) => {}
            }
        }
        if prune {
            if let Some(w) = &n.subnets {
                for l in live
                    .subnets
                    .iter()
                    .filter(|l| !w.iter().any(|x| x.id == l.id))
                {
                    deletes.push(ch("subnet", &l.id, Action::Delete, vec![l.cidr.clone()]));
                }
            }
            if let Some(w) = &n.vnets {
                for l in live
                    .vnets
                    .iter()
                    .filter(|l| !w.iter().any(|x| x.id == l.id))
                {
                    deletes.push(ch("vnet", &l.id, Action::Delete, vec![]));
                }
            }
            if let Some(w) = &n.zones {
                for l in live
                    .zones
                    .iter()
                    .filter(|l| !w.iter().any(|x| x.id == l.id))
                {
                    deletes.push(ch("zone", &l.id, Action::Delete, vec![]));
                }
            }
        }
    }

    // VMs, by name
    if let Some(vms) = &spec.vms {
        let pool_known =
            |n: &str| want_pool_names.contains(n) || live.pools.iter().any(|l| l.name == n);
        for w in vms {
            for d in &w.disks {
                if let Some(s) = d.storage.as_deref() {
                    if !pool_known(s) {
                        p.errors
                            .push(format!("vm {}: storage pool {s} does not exist", w.name));
                    }
                }
            }
            match live.vms.iter().find(|l| l.name == w.name) {
                None => {
                    creates.push(ch(
                        "vm",
                        &w.name,
                        Action::Create,
                        vec![format!("{} MB, {} cpu", w.memory, w.cpus)],
                    ));
                    if w.power == Some(Power::Running) {
                        creates.push(ch("vm", &w.name, Action::Start, vec![]));
                    }
                }
                Some(l) => {
                    let running = matches!(l.status, VmStatus::Running | VmStatus::Paused);
                    let vd = vm_diff(w, l, &live.pools);
                    let mut stopped_first = false;
                    if !vd.blocked.is_empty() {
                        let mut b = vd.blocked;
                        // the in-place part still shows, so drift lists every field
                        for f in vd.update.fields.iter() {
                            b.push(&f.field, f.expected.clone(), f.actual.clone());
                        }
                        creates.push(b.into_change("vm", &w.name, Action::Blocked));
                    } else if !vd.update.is_empty() {
                        let mut u = vd.update;
                        if running && vd.cold {
                            if w.power == Some(Power::Stopped) {
                                creates.push(ch(
                                    "vm",
                                    &w.name,
                                    Action::Stop,
                                    vec!["needed to change CPUs, memory, NICs or disks".into()],
                                ));
                                stopped_first = true;
                                creates.push(u.into_change("vm", &w.name, Action::Update));
                            } else {
                                u.note(
                                    "the VM is running: stop it first (or set power: stopped in \
                                     the spec) to change CPUs, memory, NICs or add disks",
                                );
                                creates.push(u.into_change("vm", &w.name, Action::Blocked));
                            }
                        } else {
                            creates.push(u.into_change("vm", &w.name, Action::Update));
                        }
                    }
                    match (w.power, &l.status) {
                        (Some(Power::Running), s) if *s != VmStatus::Running => {
                            creates.push(ch("vm", &w.name, Action::Start, vec![]))
                        }
                        (Some(Power::Stopped), VmStatus::Running) if !stopped_first => {
                            creates.push(ch("vm", &w.name, Action::Stop, vec![]))
                        }
                        _ => {}
                    }
                }
            }
        }
        if prune {
            for l in live
                .vms
                .iter()
                .filter(|l| !vms.iter().any(|w| w.name == l.name))
            {
                if l.status == VmStatus::Running {
                    deletes.push(ch(
                        "vm",
                        &l.name,
                        Action::Blocked,
                        vec!["prune: the VM is running; stop it first".into()],
                    ));
                } else {
                    deletes.push(ch("vm", &l.name, Action::Delete, vec![]));
                }
            }
        }
    }

    // registered kinds (containers, users, ...): one section each
    for (key, section) in &spec.extra {
        let Some(k) = kinds::find(key) else { continue };
        if let Some(e) = live.extra_errors.get(key) {
            p.errors
                .push(format!("{key}: reading the live state failed: {e}"));
            continue;
        }
        let empty = serde_json::Value::Null;
        let mut kp = kinds::KindPlan::default();
        k.plan(
            section,
            live.extra.get(key).unwrap_or(&empty),
            prune,
            (spec, live),
            &mut kp,
        );
        creates.extend(kp.changes);
        deletes.extend(kp.deletes);
        p.errors.extend(kp.errors);
    }

    // Creates/updates run dependency-first; deletes in the opposite order.
    creates.sort_by_key(|c| rank(&c.kind));
    deletes.sort_by_key(|c| std::cmp::Reverse(rank(&c.kind)));
    p.changes = creates;
    p.changes.extend(deletes);
    p
}

/// Differences between a spec VM and the live one, limited to what the spec
/// states and what Horcrux reports back reliably.
struct VmDiff {
    /// Can be changed in place.
    update: Diffs,
    /// Cannot (hypervisor, architecture, disk removal/shrink/move, ...).
    blocked: Diffs,
    /// The in-place part needs the VM stopped (CPUs, memory, NICs, new disks,
    /// disk bus); a CD-ROM swap and growing a disk work while it runs.
    cold: bool,
}

fn vm_diff(w: &VmSpec, l: &VmConfig, pools: &[crate::storage::StoragePool]) -> VmDiff {
    let mut u = Diffs::default();
    let mut b = Diffs::default();
    let mut cold = false;
    b.field("hypervisor", &l.hypervisor, &w.hypervisor);
    if let Some(a) = &w.architecture {
        b.field("architecture", &l.architecture, a);
    }
    let managed_elsewhere = l.hypervisor != VmHypervisor::Qemu;
    let (mut mem_cpu, mut other) = (Diffs::default(), Diffs::default());
    mem_cpu.field("memory", &l.memory, &w.memory);
    mem_cpu.field("cpus", &l.cpus, &w.cpus);
    if managed_elsewhere && !mem_cpu.is_empty() {
        for f in &mem_cpu.fields {
            b.push(&f.field, f.expected.clone(), f.actual.clone());
        }
        b.note("CPU and memory of LXD/Incus VMs are managed by LXD/Incus");
    } else if !mem_cpu.is_empty() {
        cold = true;
        for f in &mem_cpu.fields {
            u.push(&f.field, f.expected.clone(), f.actual.clone());
        }
    }
    if !w.disks.is_empty() {
        let live_disks: Vec<DiskSpec> = l.disks.iter().map(|x| disk_of(x, pools)).collect();
        if live_disks.len() > w.disks.len() {
            b.push(
                "disks",
                w.disks.len().to_string(),
                live_disks.len().to_string(),
            );
            b.note("removing a disk detaches data: do it by hand (Disks tab of the VM)");
        } else if live_disks.len() < w.disks.len() {
            cold = true;
            u.push(
                "disks",
                w.disks.len().to_string(),
                live_disks.len().to_string(),
            );
        }
        for (i, (a, s)) in live_disks.iter().zip(&w.disks).enumerate() {
            if s.size_gb < a.size_gb {
                b.push(
                    &format!("disk{i}.size_gb"),
                    s.size_gb.to_string(),
                    a.size_gb.to_string(),
                );
                b.note("a disk can't shrink");
            } else if s.size_gb > a.size_gb {
                u.push(
                    &format!("disk{i}.size_gb"),
                    s.size_gb.to_string(),
                    a.size_gb.to_string(),
                );
            }
            if a.bus != s.bus {
                cold = true;
                u.push(&format!("disk{i}.bus"), s.bus.clone(), a.bus.clone());
            }
            if a.storage != s.storage && s.storage.is_some() {
                b.push(
                    &format!("disk{i}.storage"),
                    show(&s.storage),
                    show(&a.storage),
                );
                b.note("an existing disk can't move to another pool in place");
            }
        }
    }
    if let Some(hw) = &w.hardware {
        let nets =
            |v: &[VmNic], keep_mac: bool| -> Vec<(String, String, Option<u16>, Option<String>)> {
                v.iter()
                    .map(|n| {
                        (
                            format!("{:?}", n.model),
                            n.bridge.clone(),
                            n.vlan,
                            if keep_mac { n.mac.clone() } else { None },
                        )
                    })
                    .collect()
            };
        // A MAC the spec leaves out is generated by the node: not a difference.
        let want_nets = nets(&hw.nets, true);
        let live_nets: Vec<_> = nets(&l.hardware.nets, true)
            .into_iter()
            .zip(&hw.nets)
            .map(|(mut a, w)| {
                if w.mac.is_none() {
                    a.3 = None;
                }
                a
            })
            .collect();
        if live_nets.len() != want_nets.len() || live_nets != want_nets {
            cold = true;
            other.push("nets", show(&want_nets), show(&live_nets));
        }
        if hw.cdrom.is_some() {
            other.field("cdrom", &l.hardware.cdrom, &hw.cdrom);
        }
    }
    for f in &other.fields {
        u.push(&f.field, f.expected.clone(), f.actual.clone());
    }
    VmDiff {
        update: u,
        blocked: b,
        cold,
    }
}

fn pool_name(pools: &[crate::storage::StoragePool], id: &str) -> String {
    pools
        .iter()
        .find(|p| p.id == id)
        .map(|p| p.name.clone())
        .unwrap_or_else(|| id.to_string())
}

fn disk_of(d: &VmDisk, pools: &[crate::storage::StoragePool]) -> DiskSpec {
    DiskSpec {
        size_gb: d.size_gb,
        bus: d.disk_type.clone(),
        cache: d.cache.clone(),
        storage: d.storage.as_deref().map(|s| pool_name(pools, s)),
        discard: d.discard,
        ssd: d.ssd,
        iothread: d.iothread,
    }
}

// ---------------------------------------------------------------- export

/// The live state as a spec (every section present, so `prune` is
/// meaningful on the result). Applying it to the same node is a no-op.
pub fn export(live: &Live) -> Spec {
    let mut pools: Vec<PoolSpec> = live
        .pools
        .iter()
        .map(|p| PoolSpec {
            name: p.name.clone(),
            storage_type: pool_type_name(&p.storage_type),
            path: p.path.clone(),
            enabled: p.enabled,
        })
        .collect();
    pools.sort_by(|a, b| a.name.cmp(&b.name));
    let mut zones = live.zones.clone();
    zones.sort_by(|a, b| a.id.cmp(&b.id));
    let mut vnets = live.vnets.clone();
    vnets.sort_by(|a, b| a.id.cmp(&b.id));
    let mut subnets = live.subnets.clone();
    subnets.sort_by(|a, b| a.id.cmp(&b.id));
    let mut vms: Vec<VmSpec> = live
        .vms
        .iter()
        .map(|v| VmSpec {
            name: v.name.clone(),
            id: Some(v.id.clone()),
            hypervisor: v.hypervisor.clone(),
            memory: v.memory,
            cpus: v.cpus,
            architecture: Some(v.architecture.clone()),
            disks: v.disks.iter().map(|d| disk_of(d, &live.pools)).collect(),
            display: (v.display != VmDisplay::default()).then_some(v.display),
            hardware: (v.hardware != VmHardware::default()).then(|| v.hardware.clone()),
            image: None,
            power: match v.status {
                VmStatus::Running => Some(Power::Running),
                VmStatus::Stopped => Some(Power::Stopped),
                _ => None,
            },
        })
        .collect();
    vms.sort_by(|a, b| a.name.cmp(&b.name));
    Spec {
        version: SPEC_VERSION,
        prune: false,
        storage_pools: Some(pools),
        networks: Some(NetworksSpec {
            zones: Some(zones),
            vnets: Some(vnets),
            subnets: Some(subnets),
        }),
        vms: Some(vms),
        extra: live
            .extra
            .iter()
            .filter(|(_, v)| !v.is_null())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    }
}

#[cfg(test)]
mod tests;
