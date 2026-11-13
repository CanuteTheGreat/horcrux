use super::*;
use crate::storage::{StoragePool, StorageType};

const SPEC: &str = r#"
version: 1
storage_pools:
  - {name: fast, type: directory, path: /srv/fast}
networks:
  zones:
    - {id: lab, type: simple}
  vnets:
    - {id: web, zone: lab}
  subnets:
    - {vnet: web, cidr: 10.20.0.0/24, gateway: 10.20.0.1, snat: true}
vms:
  - name: app1
    memory: 512
    cpus: 2
    disks: [{size_gb: 8, storage: fast}]
    hardware:
      nets: [{bridge: web}]
    power: running
"#;

fn pool(id: &str, name: &str) -> StoragePool {
    StoragePool {
        id: id.into(),
        name: name.into(),
        storage_type: StorageType::Directory,
        path: format!("/srv/{name}"),
        available: 5,
        total: 10,
        enabled: true,
    }
}

fn live_vm(name: &str, status: VmStatus) -> VmConfig {
    VmConfig {
        id: name.into(),
        name: name.into(),
        hypervisor: VmHypervisor::Qemu,
        memory: 512,
        cpus: 2,
        disk_size: 8,
        status,
        architecture: VmArchitecture::default(),
        disks: vec![],
        display: VmDisplay::default(),
        hardware: VmHardware::default(),
    }
}

fn actions(p: &Plan) -> Vec<String> {
    p.changes
        .iter()
        .map(|c| format!("{:?} {} {}", c.action, c.kind, c.name))
        .collect()
}

#[test]
fn parses_yaml_and_json_and_derives_subnet_ids() {
    let s = parse(SPEC).unwrap();
    assert_eq!(
        s.networks.as_ref().unwrap().subnets.as_ref().unwrap()[0].id,
        "web-10.20.0.0-24"
    );
    assert_eq!(s.vms.as_ref().unwrap()[0].vm_id(), "app1");
    let j = serde_json::to_string(&s).unwrap();
    assert_eq!(parse(&j).unwrap(), s);
}

#[test]
fn rejects_bad_specs_with_clear_messages() {
    for (text, want) in [
        ("version: 2", "not supported"),
        ("containers: []", "not supported yet"),
        (
            "storage_pools: [{name: a, type: floppy, path: /x}]",
            "unknown storage type",
        ),
        (
            "vms: [{name: a, memory: 1}, {name: a, memory: 1}]",
            "listed twice",
        ),
        ("vms: [{name: 'a b', memory: 1}]", "VM"),
        ("vms: [{name: a, memory: 0}]", "> 0"),
        ("networks: {zones: [{id: z, type: vlan}]}", "uplink"),
        (
            "networks: {subnets: [{vnet: a, cidr: 10.0.0.0/24}, {vnet: b, cidr: 10.0.0.0/25}]}",
            "overlap",
        ),
    ] {
        let e = parse(text).unwrap_err();
        assert!(e.contains(want), "{text}: {e}");
    }
}

#[test]
fn empty_node_plans_everything_in_dependency_order() {
    let s = parse(SPEC).unwrap();
    let p = plan(&s, &Live::default(), false);
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    assert_eq!(
        actions(&p),
        [
            "Create storage_pool fast",
            "Create zone lab",
            "Create vnet web",
            "Create subnet web-10.20.0.0-24",
            "Create vm app1",
            "Start vm app1"
        ]
    );
}

#[test]
fn dangling_references_are_plan_errors() {
    let s = parse(
        "networks: {vnets: [{id: v, zone: nozone}]}\nvms: [{name: a, memory: 1, disks: [{size_gb: 1, storage: nopool}]}]",
    )
    .unwrap();
    let p = plan(&s, &Live::default(), false);
    assert_eq!(p.errors.len(), 2, "{:?}", p.errors);
    // a live pool satisfies the reference
    let live = Live {
        pools: vec![pool("p1", "nopool")],
        ..Default::default()
    };
    assert_eq!(plan(&s, &live, false).errors.len(), 1);
}

#[test]
fn export_then_plan_is_in_sync_and_survives_yaml() {
    let live = Live {
        pools: vec![pool("p1", "fast")],
        zones: vec![Zone {
            id: "lab".into(),
            zone_type: "simple".into(),
            description: String::new(),
            uplink: None,
            local_ip: None,
            peers: vec![],
            mtu: None,
            dst_port: None,
        }],
        vms: vec![live_vm("app1", VmStatus::Running)],
        ..Default::default()
    };
    let spec = export(&live);
    let yaml = serde_yaml::to_string(&spec).unwrap();
    let back = parse(&yaml).unwrap();
    assert!(
        plan(&back, &live, true).in_sync(),
        "{:?}",
        plan(&back, &live, true)
    );
}

#[test]
fn unmanaged_sections_and_unlisted_resources_are_left_alone() {
    let live = Live {
        pools: vec![pool("p1", "old")],
        vms: vec![live_vm("legacy", VmStatus::Stopped)],
        ..Default::default()
    };
    // only vms managed, no prune: nothing happens to the pool or "legacy"
    let s = parse("vms: [{name: new, memory: 64}]").unwrap();
    assert_eq!(actions(&plan(&s, &live, false)), ["Create vm new"]);
    // prune deletes unlisted VMs, but the absent storage_pools section is untouched
    assert_eq!(
        actions(&plan(&s, &live, true)),
        ["Create vm new", "Delete vm legacy"]
    );
    // a present, empty section means "none should exist"
    let s = parse("storage_pools: []").unwrap();
    assert_eq!(actions(&plan(&s, &live, true)), ["Delete storage_pool old"]);
    assert!(plan(&s, &live, false).in_sync());
}

#[test]
fn prune_never_deletes_a_running_vm() {
    let live = Live {
        vms: vec![live_vm("busy", VmStatus::Running)],
        ..Default::default()
    };
    let p = plan(&parse("vms: []").unwrap(), &live, true);
    assert_eq!(actions(&p), ["Blocked vm busy"]);
}

#[test]
fn deletes_run_dependents_first() {
    let live = Live {
        pools: vec![pool("p1", "a")],
        vms: vec![live_vm("x", VmStatus::Stopped)],
        ..Default::default()
    };
    let p = plan(&parse("storage_pools: []\nvms: []").unwrap(), &live, true);
    assert_eq!(actions(&p), ["Delete vm x", "Delete storage_pool a"]);
}

#[test]
fn drift_on_existing_vm_is_reported_and_power_is_reconciled() {
    let live = Live {
        vms: vec![live_vm("app1", VmStatus::Stopped)],
        ..Default::default()
    };
    let s = parse("vms: [{name: app1, memory: 1024, cpus: 2, power: running}]").unwrap();
    let p = plan(&s, &live, false);
    assert_eq!(actions(&p), ["Blocked vm app1", "Start vm app1"]);
    assert!(p.changes[0].details[0].starts_with("memory: 512 -> 1024"));
    // same config, only a different power state
    let s = parse("vms: [{name: app1, memory: 512, cpus: 2, power: stopped}]").unwrap();
    assert!(plan(&s, &live, false).in_sync());
}

#[test]
fn generated_macs_are_not_drift() {
    let mut vm = live_vm("app1", VmStatus::Stopped);
    vm.hardware.nets = vec![VmNic {
        model: Default::default(),
        bridge: "br0".into(),
        mac: Some("52:54:00:aa:bb:cc".into()),
        vlan: None,
    }];
    let live = Live {
        vms: vec![vm],
        ..Default::default()
    };
    let s = parse("vms: [{name: app1, memory: 512, cpus: 2, hardware: {nets: [{bridge: br0}]}}]")
        .unwrap();
    assert!(plan(&s, &live, false).in_sync());
    let s = parse("vms: [{name: app1, memory: 512, cpus: 2, hardware: {nets: [{bridge: br1}]}}]")
        .unwrap();
    assert_eq!(actions(&plan(&s, &live, false)), ["Blocked vm app1"]);
}

#[test]
fn immutable_network_and_pool_changes_are_blocked_not_silently_dropped() {
    let s = parse(SPEC).unwrap();
    let mut live = Live::default();
    live.pools.push(pool("p1", "fast")); // path /srv/fast: same
    live.vnets.push(VNet {
        id: "web".into(),
        zone: "lab".into(),
        alias: None,
        tag: Some(5),
        vlan_aware: false,
    });
    let p = plan(&s, &live, false);
    assert!(
        actions(&p).contains(&"Blocked vnet web".to_string()),
        "{:?}",
        actions(&p)
    );
    assert!(!actions(&p).iter().any(|a| a.contains("storage_pool")));
}
