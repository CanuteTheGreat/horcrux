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
        source: None,
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
        attach: Default::default(),
        mac: Some("52:54:00:aa:bb:cc".into()),
        vlan: None,
        rate_mbps: None,
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

#[test]
fn console_settings_are_planned_validated_and_exported() {
    use horcrux_common::console::{ConsoleProfile, ConsoleSettings};
    let live = Live::default();
    // absent section: unmanaged
    assert!(plan(&Spec::default(), &live, false).in_sync());
    // same as live: in sync
    let mut s = Spec {
        console: Some(ConsoleSettings::default()),
        ..Default::default()
    };
    assert!(plan(&s, &live, false).in_sync());
    // a change is one update with key-level details
    let mut want = ConsoleSettings {
        default_profile: ConsoleProfile::Gaming,
        ..Default::default()
    };
    want.desktop.max_fps = 60;
    s.console = Some(want.clone());
    let p = plan(&s, &live, false);
    assert_eq!(p.changes.len(), 1);
    assert_eq!(p.changes[0].kind, "console");
    assert_eq!(p.changes[0].action, Action::Update);
    let d = p.changes[0].details.join("\n");
    assert!(
        d.contains("default_profile: \"desktop\" -> \"gaming\""),
        "{d}"
    );
    assert!(d.contains("desktop.max_fps: 30 -> 60"), "{d}");
    // invalid values are plan errors, not changes
    want.codecs = vec!["mpeg2".into()];
    s.console = Some(want);
    assert!(plan(&s, &live, false).errors[0].contains("mpeg2"));
    // export carries the live settings and round-trips through YAML
    let e = export(&live);
    let y = serde_yaml::to_string(&e).unwrap();
    assert!(y.contains("console:"), "{y}");
    let back: Spec = serde_yaml::from_str(&y).unwrap();
    assert!(plan(&back, &live, false).in_sync());
}

#[test]
fn remote_play_settings_are_planned_validated_and_exported() {
    use horcrux_common::remote_play::RemotePlaySettings;
    let live = Live::default();
    let mut s = Spec {
        remote_play: Some(RemotePlaySettings::default()),
        ..Default::default()
    };
    assert!(plan(&s, &live, false).in_sync());
    let want = RemotePlaySettings {
        wol_ports: vec![9],
        ..Default::default()
    };
    s.remote_play = Some(want.clone());
    let p = plan(&s, &live, false);
    assert_eq!(p.changes.len(), 1);
    assert_eq!(p.changes[0].kind, "remote_play");
    assert!(
        p.changes[0]
            .details
            .join("\n")
            .contains("wol_ports: [9,7] -> [9]"),
        "{:?}",
        p.changes[0].details
    );
    s.remote_play = Some(RemotePlaySettings {
        wol_ports: vec![],
        ..Default::default()
    });
    assert!(plan(&s, &live, false).errors[0].contains("wol_ports"));
    let y = serde_yaml::to_string(&export(&live)).unwrap();
    assert!(y.contains("remote_play:"), "{y}");
    let back: Spec = serde_yaml::from_str(&y).unwrap();
    assert!(plan(&back, &live, false).in_sync());
}

fn gp_actions(p: &Plan) -> Vec<String> {
    actions(p)
        .into_iter()
        .filter(|a| a.contains("guest_power"))
        .collect()
}

#[test]
fn guest_power_is_planned_exported_and_pruned() {
    use crate::api::guest_state::{GuestOverride, StopAction};
    let s = parse(
        "guest_power:\n  node: {host_shutdown_action: shutdown, save_timeout_secs: 90}\n  vms:\n    app1: {host_shutdown_action: none}\nvms: [{name: app1, memory: 512}]",
    )
    .unwrap();
    let gp = s.guest_power.as_ref().unwrap();
    assert_eq!(gp.node.stop_action, StopAction::Shutdown);
    assert_eq!(gp.node.save_timeout_secs, 90);
    let live = Live {
        vms: vec![live_vm("app1", VmStatus::Stopped)],
        ..Default::default()
    };
    let p = plan(&s, &live, false);
    let a = actions(&p);
    assert!(
        a.contains(&format!("Update guest_power {NODE_POWER}")),
        "{a:?}"
    );
    assert!(a.contains(&"Update guest_power app1".to_string()), "{a:?}");
    // an override on a VM the spec no longer lists is only removed with prune
    let mut live2 = live.clone();
    live2.power_node = gp.node.clone();
    live2.power_vms.insert(
        "app1".into(),
        GuestOverride {
            stop_action: Some(StopAction::None),
            ..Default::default()
        },
    );
    assert!(gp_actions(&plan(&s, &live2, true)).is_empty());
    let s2 = parse("guest_power: {node: {}}\nvms: [{name: app1, memory: 512}]").unwrap();
    let mut live3 = live2.clone();
    live3.power_node = Default::default();
    assert!(gp_actions(&plan(&s2, &live3, false)).is_empty());
    assert!(actions(&plan(&s2, &live3, true)).contains(&"Delete guest_power app1".to_string()));
    // export then plan is a no-op and survives YAML
    let exp = export(&live2);
    let back = parse(&serde_yaml::to_string(&exp).unwrap()).unwrap();
    assert!(plan(&back, &live2, true).in_sync());
}

#[test]
fn guest_power_rejects_unknown_vms_and_bad_values() {
    let s = parse("guest_power: {vms: {ghost: {host_shutdown_action: save}}}").unwrap();
    assert_eq!(plan(&s, &Live::default(), false).errors.len(), 1);
    assert!(parse("guest_power: {node: {max_parallel: 0}}").is_err());
    assert!(parse("guest_power: {vms: {a: {save_dir: relative}}}").is_err());
}

#[test]
fn notification_matchers_are_planned_validated_pruned_and_exported() {
    use horcrux_common::notify::{MatchNode, Matcher, Severity};
    let live_m = Matcher {
        id: "x1".into(),
        name: "old".into(),
        targets: vec!["hook".into()],
        all_channels: false,
        ..Matcher::default_matcher()
    };
    let live = Live {
        matchers: vec![live_m.clone()],
        channel_names: vec!["hook".into(), "mail".into()],
        ..Default::default()
    };
    // unchanged (ids are not part of the spec) -> no change
    let mut same = live_m.clone();
    same.id = String::new();
    let s = Spec {
        notification_matchers: Some(vec![same.clone()]),
        ..Default::default()
    };
    assert!(plan(&s, &live, false).changes.is_empty());
    // a new matcher and a changed one
    let mut changed = same.clone();
    changed.invert = true;
    let fresh = Matcher {
        name: "warnings".into(),
        children: vec![MatchNode::Severity {
            invert: false,
            levels: vec![],
            min: Some(Severity::Warning),
        }],
        targets: vec!["mail".into()],
        all_channels: false,
        ..Matcher::default_matcher()
    };
    let s = Spec {
        notification_matchers: Some(vec![changed, fresh.clone()]),
        ..Default::default()
    };
    let p = plan(&s, &live, false);
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    let got: Vec<_> = p
        .changes
        .iter()
        .map(|c| (c.kind.as_str(), c.name.as_str(), c.action))
        .collect();
    assert!(
        got.contains(&("notification_matcher", "old", Action::Update)),
        "{got:?}"
    );
    assert!(
        got.contains(&("notification_matcher", "warnings", Action::Create)),
        "{got:?}"
    );
    // prune removes matchers the spec lacks
    let s = Spec {
        notification_matchers: Some(vec![fresh.clone()]),
        ..Default::default()
    };
    let p = plan(&s, &live, true);
    assert!(p
        .changes
        .iter()
        .any(|c| c.name == "old" && c.action == Action::Delete));
    assert!(!plan(&s, &live, false)
        .changes
        .iter()
        .any(|c| c.action == Action::Delete));
    // a target that is not a channel, and a bad regex, are plan errors
    let mut bad = fresh.clone();
    bad.targets = vec!["nope".into()];
    bad.children = vec![MatchNode::Field {
        invert: false,
        field: "title".into(),
        regex: "(".into(),
    }];
    let s = Spec {
        notification_matchers: Some(vec![bad]),
        ..Default::default()
    };
    let e = plan(&s, &live, false).errors.join(";");
    assert!(
        e.contains("not a notification channel") && e.contains("invalid regex"),
        "{e}"
    );
    // export carries them, without ids, and round-trips through YAML
    let ex = export(&live);
    let y = serde_yaml::to_string(&ex).unwrap();
    assert!(y.contains("notification_matchers:"), "{y}");
    let back: Spec = serde_yaml::from_str(&y).unwrap();
    assert_eq!(back.notification_matchers.unwrap()[0].id, "");
}
