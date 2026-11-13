//! Declarative cluster configuration: a spec file describes the desired
//! state, `plan` diffs it against the live node, `apply` makes it so and
//! `export` dumps the live state as a spec. See
//! `docs/design/declarative-config.md`.
//!
//! This file is pure (no I/O, no init system, no kernel): spec types,
//! validation, diff and export. `exec.rs` talks to the managers; the same
//! code therefore behaves identically on OpenRC and systemd nodes.
pub mod exec;

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
    /// Sections reserved for later phases: rejected with a clear message.
    #[serde(flatten, skip_serializing)]
    pub extra: BTreeMap<String, serde_yaml::Value>,
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
    if let Some(k) = spec.extra.keys().next() {
        return Err(format!(
            "section {k:?} is not supported yet (phase 1 manages storage_pools, networks and vms)"
        ));
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
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Create,
    Update,
    Delete,
    Start,
    Stop,
    /// Differs, but phase 1 cannot change it in place (details say why).
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Change {
    /// storage_pool | zone | vnet | subnet | vm
    pub kind: String,
    pub name: String,
    pub action: Action,
    pub details: Vec<String>,
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
}

fn ch(kind: &str, name: &str, action: Action, details: Vec<String>) -> Change {
    Change {
        kind: kind.into(),
        name: name.into(),
        action,
        details,
    }
}

fn diff<T: PartialEq + std::fmt::Debug>(out: &mut Vec<String>, what: &str, live: &T, want: &T) {
    if live != want {
        out.push(format!("{what}: {live:?} -> {want:?}"));
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
                let mut d = Vec::new();
                diff(
                    &mut d,
                    "type",
                    &pool_type_name(&l.storage_type),
                    &pool_type_name(&pool_type(&w.storage_type).unwrap_or(l.storage_type.clone())),
                );
                diff(&mut d, "path", &l.path, &w.path);
                diff(&mut d, "enabled", &l.enabled, &w.enabled);
                if !d.is_empty() {
                    d.push("pools can't be edited in place yet: remove and re-add it".into());
                    creates.push(ch("storage_pool", &w.name, Action::Blocked, d));
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
                    let mut d = Vec::new();
                    diff(&mut d, "type", &l.zone_type, &w.zone_type);
                    diff(&mut d, "description", &l.description, &w.description);
                    diff(&mut d, "uplink", &l.uplink, &w.uplink);
                    diff(&mut d, "local_ip", &l.local_ip, &w.local_ip);
                    diff(&mut d, "peers", &l.peers, &w.peers);
                    diff(&mut d, "mtu", &l.mtu, &w.mtu);
                    diff(&mut d, "dst_port", &l.dst_port, &w.dst_port);
                    let blocked =
                        l.zone_type != w.zone_type && live.vnets.iter().any(|v| v.zone == w.id);
                    if blocked {
                        d.push("the zone type can't change while VNets use the zone".into());
                    }
                    creates.push(ch(
                        "zone",
                        &w.id,
                        if blocked {
                            Action::Blocked
                        } else {
                            Action::Update
                        },
                        d,
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
                    let mut d = Vec::new();
                    diff(&mut d, "zone", &l.zone, &w.zone);
                    diff(&mut d, "alias", &l.alias, &w.alias);
                    diff(&mut d, "tag", &l.tag, &w.tag);
                    diff(&mut d, "vlan_aware", &l.vlan_aware, &w.vlan_aware);
                    d.push("VNets can't be edited in place yet: delete and re-create it".into());
                    creates.push(ch("vnet", &w.id, Action::Blocked, d));
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
                    let mut d = Vec::new();
                    diff(&mut d, "gateway", &l.gateway, &w.gateway);
                    diff(&mut d, "snat", &l.snat, &w.snat);
                    d.push("subnets can't be edited in place yet: delete and re-create it".into());
                    creates.push(ch("subnet", &w.id, Action::Blocked, d));
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
        let vnet_ids: BTreeSet<&str> = spec
            .networks
            .iter()
            .flat_map(|n| n.vnets.iter().flatten())
            .map(|v| v.id.as_str())
            .collect();
        for w in vms {
            for d in &w.disks {
                if let Some(s) = d.storage.as_deref() {
                    if !pool_known(s) {
                        p.errors
                            .push(format!("vm {}: storage pool {s} does not exist", w.name));
                    }
                }
            }
            let _ = &vnet_ids; // bridges may also be host bridges: checked at apply time
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
                    let d = vm_diff(w, l, &live.pools);
                    if !d.is_empty() {
                        let mut d = d;
                        d.push("VMs can't be reconfigured in place yet".into());
                        creates.push(ch("vm", &w.name, Action::Blocked, d));
                    }
                    match (w.power, &l.status) {
                        (Some(Power::Running), s) if *s != VmStatus::Running => {
                            creates.push(ch("vm", &w.name, Action::Start, vec![]))
                        }
                        (Some(Power::Stopped), VmStatus::Running) => {
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

    // Creates/updates run dependency-first (pools, zones, vnets, subnets,
    // VMs); deletes run in the opposite order (VMs first, pools last).
    let rank = |k: &str| {
        ["storage_pool", "zone", "vnet", "subnet", "vm"]
            .iter()
            .position(|x| *x == k)
            .unwrap_or(9)
    };
    creates.sort_by_key(|c| rank(&c.kind));
    deletes.sort_by_key(|c| std::cmp::Reverse(rank(&c.kind)));
    p.changes = creates;
    p.changes.extend(deletes);
    p
}

/// Differences between a spec VM and the live one, limited to what the spec
/// states and what Horcrux reports back reliably.
fn vm_diff(w: &VmSpec, l: &VmConfig, pools: &[crate::storage::StoragePool]) -> Vec<String> {
    let mut d = Vec::new();
    diff(&mut d, "hypervisor", &l.hypervisor, &w.hypervisor);
    diff(&mut d, "memory", &l.memory, &w.memory);
    diff(&mut d, "cpus", &l.cpus, &w.cpus);
    if let Some(a) = &w.architecture {
        diff(&mut d, "architecture", &l.architecture, a);
    }
    if !w.disks.is_empty() {
        let live_disks: Vec<DiskSpec> = l.disks.iter().map(|x| disk_of(x, pools)).collect();
        if live_disks.len() != w.disks.len() {
            d.push(format!("disks: {} -> {}", live_disks.len(), w.disks.len()));
        } else {
            for (i, (a, b)) in live_disks.iter().zip(&w.disks).enumerate() {
                diff(&mut d, &format!("disk{i}.size_gb"), &a.size_gb, &b.size_gb);
                diff(&mut d, &format!("disk{i}.bus"), &a.bus, &b.bus);
                diff(&mut d, &format!("disk{i}.storage"), &a.storage, &b.storage);
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
            d.push(format!("nets: {live_nets:?} -> {want_nets:?}"));
        }
        if hw.cdrom.is_some() {
            diff(&mut d, "cdrom", &l.hardware.cdrom, &hw.cdrom);
        }
    }
    d
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
        extra: BTreeMap::new(),
    }
}

#[cfg(test)]
mod tests;
