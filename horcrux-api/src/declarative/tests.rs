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
        ("frobnicators: []", "not supported yet"),
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
    // memory is now changed in place (the VM is stopped)
    assert_eq!(actions(&p), ["Update vm app1", "Start vm app1"]);
    assert!(p.changes[0].details[0].starts_with("memory: 512 -> 1024"));
    assert_eq!(p.changes[0].fields[0].field, "memory");
    assert_eq!(p.changes[0].fields[0].expected, "1024");
    assert_eq!(p.changes[0].fields[0].actual, "512");
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
    assert_eq!(actions(&plan(&s, &live, false)), ["Update vm app1"]);
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
    // a tag/alias edit is in place now; moving the VNet to another zone is not
    assert!(
        actions(&p).contains(&"Update vnet web".to_string()),
        "{:?}",
        actions(&p)
    );
    live.vnets[0].zone = "other".into();
    let p = plan(&s, &live, false);
    assert!(
        actions(&p).contains(&"Blocked vnet web".to_string()),
        "{:?}",
        actions(&p)
    );
    assert!(!actions(&p).iter().any(|a| a.contains("storage_pool")));
    // a pool's path can't change in place; enabled can
    live.pools[0].path = "/elsewhere".into();
    assert!(actions(&plan(&s, &live, false)).contains(&"Blocked storage_pool fast".to_string()));
    live.pools[0].path = "/srv/fast".into();
    live.pools[0].enabled = false;
    assert!(actions(&plan(&s, &live, false)).contains(&"Update storage_pool fast".to_string()));
}

fn live_disk(size: u64, bus: &str) -> VmDisk {
    VmDisk {
        path: "/x".into(),
        size_gb: size,
        disk_type: bus.into(),
        cache: String::new(),
        discard: false,
        ssd: false,
        iothread: false,
        storage: None,
    }
}

#[test]
fn running_vm_cold_changes_need_a_stop_or_are_blocked_with_a_reason() {
    let live = Live {
        vms: vec![live_vm("app1", VmStatus::Running)],
        ..Default::default()
    };
    // memory change on a running VM, power left alone: blocked, says why
    let s = parse("vms: [{name: app1, memory: 1024, cpus: 2}]").unwrap();
    let p = plan(&s, &live, false);
    assert_eq!(actions(&p), ["Blocked vm app1"]);
    assert!(p.changes[0]
        .details
        .iter()
        .any(|d| d.contains("stop it first")));
    assert_eq!(p.changes[0].fields[0].field, "memory");
    // power: stopped in the spec makes the engine do the stop itself
    let s = parse("vms: [{name: app1, memory: 1024, cpus: 2, power: stopped}]").unwrap();
    let p = plan(&s, &live, false);
    assert_eq!(actions(&p), ["Stop vm app1", "Update vm app1"]);
}

#[test]
fn disk_growth_is_hot_shrink_and_removal_are_blocked_new_disk_is_cold() {
    let mut vm = live_vm("app1", VmStatus::Running);
    vm.disks = vec![live_disk(8, "virtio"), live_disk(4, "virtio")];
    let live = Live {
        vms: vec![vm],
        ..Default::default()
    };
    let grow =
        parse("vms: [{name: app1, memory: 512, cpus: 2, disks: [{size_gb: 16}, {size_gb: 4}]}]")
            .unwrap();
    let p = plan(&grow, &live, false);
    assert_eq!(
        actions(&p),
        ["Update vm app1"],
        "growing works while running"
    );
    assert_eq!(p.changes[0].fields[0].field, "disk0.size_gb");
    assert_eq!(p.changes[0].fields[0].actual, "8");
    let shrink =
        parse("vms: [{name: app1, memory: 512, cpus: 2, disks: [{size_gb: 2}, {size_gb: 4}]}]")
            .unwrap();
    assert_eq!(actions(&plan(&shrink, &live, false)), ["Blocked vm app1"]);
    let fewer = parse("vms: [{name: app1, memory: 512, cpus: 2, disks: [{size_gb: 8}]}]").unwrap();
    let p = plan(&fewer, &live, false);
    assert_eq!(actions(&p), ["Blocked vm app1"]);
    assert!(p.changes[0]
        .details
        .iter()
        .any(|d| d.contains("detaches data")));
    let more = parse(
        "vms: [{name: app1, memory: 512, cpus: 2, disks: [{size_gb: 8}, {size_gb: 4}, {size_gb: 1}]}]",
    )
    .unwrap();
    // a new disk is a cold change: the VM is running, so it is blocked
    assert_eq!(actions(&plan(&more, &live, false)), ["Blocked vm app1"]);
}

#[test]
fn subnet_and_vnet_edits_are_updates() {
    let s = parse(SPEC).unwrap();
    let mut live = Live::default();
    live.pools.push(pool("p1", "fast"));
    live.zones.push(Zone {
        id: "lab".into(),
        zone_type: "simple".into(),
        description: String::new(),
        uplink: None,
        local_ip: None,
        peers: vec![],
        mtu: None,
        dst_port: None,
    });
    live.vnets.push(VNet {
        id: "web".into(),
        zone: "lab".into(),
        alias: Some("old".into()),
        tag: None,
        vlan_aware: false,
    });
    live.subnets.push(Subnet {
        id: "web-10.20.0.0-24".into(),
        vnet: "web".into(),
        cidr: "10.20.0.0/24".into(),
        gateway: Some("10.20.0.254".into()),
        snat: false,
    });
    let p = plan(&s, &live, false);
    let a = actions(&p);
    assert!(a.contains(&"Update vnet web".to_string()), "{a:?}");
    assert!(
        a.contains(&"Update subnet web-10.20.0.0-24".to_string()),
        "{a:?}"
    );
    let sub = p.changes.iter().find(|c| c.kind == "subnet").unwrap();
    let f: Vec<_> = sub
        .fields
        .iter()
        .map(|f| (f.field.as_str(), f.expected.as_str(), f.actual.as_str()))
        .collect();
    assert_eq!(
        f,
        [
            ("gateway", "10.20.0.1", "10.20.0.254"),
            ("snat", "true", "false")
        ]
    );
}

#[test]
fn drift_items_flatten_plans_per_field() {
    let live = Live {
        vms: vec![live_vm("app1", VmStatus::Stopped)],
        ..Default::default()
    };
    let s = parse("storage_pools: [{name: fast, type: directory, path: /srv/fast}]\nvms: [{name: app1, memory: 1024, cpus: 2, power: running}]").unwrap();
    let items = drift::items_of(&plan(&s, &live, false));
    let f: Vec<_> = items
        .iter()
        .map(|i| {
            format!(
                "{}/{}/{}: {} vs {}",
                i.kind, i.name, i.field, i.expected, i.actual
            )
        })
        .collect();
    assert_eq!(
        f,
        [
            "storage_pool/fast/(resource): present vs missing",
            "vm/app1/memory: 1024 vs 512",
            "vm/app1/power: running vs stopped",
        ]
    );
    assert!(items.iter().all(|i| !i.blocked));
}

#[test]
fn rollback_helpers_and_secret_free_history_summary() {
    // the history summary counts non-applied results as failed
    let e = store::HistoryEntry {
        id: "0000000000001-aaaa".into(),
        time: store::now(),
        ts_ms: 1,
        actor: "admin".into(),
        source: "api".into(),
        commit: None,
        spec_hash: store::sha256_hex("x"),
        spec: "x".into(),
        prune: false,
        ok: false,
        plan: Plan::default(),
        results: vec![
            exec::ChangeResult {
                change: ch("vm", "a", Action::Create, vec![]),
                status: "applied".into(),
                message: String::new(),
            },
            exec::ChangeResult {
                change: ch("vm", "b", Action::Create, vec![]),
                status: "failed".into(),
                message: "no".into(),
            },
        ],
        rollback_of: None,
        summary: "1 applied, 1 failed".into(),
    };
    let s = e.summarize();
    assert_eq!((s.failed, s.ok), (1, false));
    assert_eq!(store::sha256_hex("x").len(), 64);
}

// ---------------------------------------------------------------- kinds

#[allow(clippy::field_reassign_with_default)]
mod kinds_integration {
    use super::*;
    use crate::declarative::kinds::access::CallerInfo;
    use crate::declarative::kinds::ctx::{Ctx, UserPriv};
    use serde_json::json;
    use std::collections::BTreeMap;

    const HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$aaaa$bbbb";

    fn pw_file() -> String {
        let p = std::env::temp_dir().join(format!("hx-decl-kinds-pw-{}", std::process::id()));
        std::fs::write(&p, "a-secret-pw-1\n").unwrap();
        p.display().to_string()
    }

    fn spec_text() -> String {
        format!(
            r#"
version: 1
containers:
  - {{name: web, image: nginx, ports: ["8080:80"], restart: always, power: running}}
volumes:
  - {{name: data}}
roles:
  - {{name: Operator, privileges: [VM.PowerMgmt]}}
users:
  - {{username: admin}}
  - {{username: alice, password_ref: {{file: {}}}}}
groups:
  - {{name: ops, members: [alice]}}
acl:
  - {{path: /vms, subject_type: group, subject: ops, role: Operator}}
firewall:
  aliases: [{{name: office, cidr: 192.0.2.0/24}}]
  rules:
    - {{scope: datacenter, id: ssh, action: ACCEPT, protocol: tcp, source: office, dport: "22"}}
backup_jobs:
  - {{id: nightly, schedule: "0 2 * * *", targets: [web]}}
ha_groups:
  - {{name: g1, nodes: [n1]}}
"#,
            pw_file()
        )
    }

    /// A node with one administrator and nothing else, as the kinds' `gather`
    /// reports it (sections spec-shaped, the rest in `Live::kinds`).
    fn empty_node() -> Live {
        let mut live = Live::default();
        live.kinds = Ctx {
            caller: Some(CallerInfo {
                user_id: "id-admin".into(),
                username: "admin".into(),
                is_admin: true,
            }),
            api_port: 8006,
            users: BTreeMap::from([(
                "admin".to_string(),
                UserPriv {
                    id: "id-admin".into(),
                    hash: HASH.into(),
                },
            )]),
            builtin_roles: vec![(
                "Administrator".into(),
                vec!["Permissions.Modify".into(), "Sys.Modify".into()],
            )],
            ..Default::default()
        };
        for (k, v) in [
            ("containers", json!([])),
            ("volumes", json!([])),
            ("roles", json!([])),
            ("users", json!([{"username": "admin"}])),
            ("groups", json!([])),
            (
                "acl",
                json!([{"path": "/", "subject_type": "user", "subject": "admin", "role": "Administrator"}]),
            ),
            (
                "firewall",
                json!({"options": {"policy_in": "accept", "policy_out": "accept"},
                       "aliases": [], "ipsets": [], "security_groups": [], "rules": []}),
            ),
            ("backup_jobs", json!([])),
            ("ha_groups", json!([])),
        ] {
            live.extra.insert(k.to_string(), v);
        }
        live
    }

    #[test]
    fn every_kind_parses_validates_and_roundtrips_through_yaml_and_json() {
        let s = parse(&spec_text()).unwrap();
        for k in [
            "containers",
            "volumes",
            "roles",
            "users",
            "groups",
            "acl",
            "firewall",
            "backup_jobs",
            "ha_groups",
        ] {
            assert!(s.extra.contains_key(k), "{k}");
        }
        assert_eq!(parse(&serde_yaml::to_string(&s).unwrap()).unwrap(), s);
        assert_eq!(parse(&serde_json::to_string(&s).unwrap()).unwrap(), s);
        assert_eq!(kinds::keys().len(), 9);
    }

    #[test]
    fn bad_kind_sections_are_rejected_with_the_reason() {
        for (text, want) in [
            ("containers: [{name: a, image: x, memroy: 5}]", "memroy"),
            (
                "containers: [{name: a, image: x, env: {DB_PASSWORD: p}}]",
                "secret",
            ),
            ("users: [{username: a, password: hunter2}]", "password"),
            ("users: [{username: a, password_hash: hunter2}]", "argon2"),
            ("firewall: {rules: [{id: a, action: ACCEPT}]}", "scope"),
            (
                "backup_jobs: [{id: a, schedule: daily, targets: []}]",
                "target",
            ),
            ("ha_groups: [{name: a, nodes: []}]", "node"),
            ("volumes: [{name: -x}]", "volume"),
            ("nodes: {}", "not supported yet"),
        ] {
            let e = parse(text).unwrap_err();
            assert!(e.contains(want), "{text}: {e}");
        }
    }

    #[test]
    fn plan_orders_the_kinds_dependency_first_with_the_firewall_reload_last() {
        let s = parse(&spec_text()).unwrap();
        let p = plan(&s, &empty_node(), false);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        let mut order: Vec<String> = p.changes.iter().map(|c| c.kind.clone()).collect();
        order.dedup();
        assert_eq!(
            order,
            [
                "role",
                "user",
                "group",
                "acl",
                "volume",
                "container",
                "fw_alias",
                "fw_rule",
                "backup_job",
                "ha_group",
                "fw_reload"
            ]
        );
        assert_eq!(
            actions(&p)
                .iter()
                .filter(|a| a.ends_with("container web"))
                .collect::<Vec<_>>(),
            ["Create container web", "Start container web"]
        );
    }

    #[test]
    fn a_password_reference_that_cannot_be_read_is_a_plan_error() {
        let s = parse(
            "users: [{username: admin}, {username: bob, password_ref: {file: /nonexistent/hx-secret}}]",
        )
        .unwrap();
        let p = plan(&s, &empty_node(), false);
        assert!(
            p.errors.iter().any(|e| e.contains("hx-secret")),
            "{:?}",
            p.errors
        );
    }

    #[test]
    fn prune_deletes_the_kinds_in_reverse_dependency_order_and_only_in_listed_sections() {
        let mut live = empty_node();
        live.extra
            .insert("ha_groups".into(), json!([{"name": "old", "nodes": ["n"]}]));
        live.extra.insert(
            "containers".into(),
            json!([{"name": "c", "image": "x:1", "runtime": "podman", "power": "stopped"}]),
        );
        let s = parse("ha_groups: []\ncontainers: []\nvms: []").unwrap();
        let p = plan(&s, &live, true);
        assert_eq!(actions(&p), ["Delete ha_group old", "Delete container c"]);
        // sections the spec doesn't list are untouched even with prune
        let p = plan(&parse("vms: []").unwrap(), &live, true);
        assert!(p.changes.is_empty());
    }

    #[test]
    fn a_section_whose_live_state_cannot_be_read_is_a_plan_error_not_a_silent_skip() {
        let mut live = empty_node();
        live.extra.remove("backup_jobs");
        live.extra_errors.insert(
            "backup_jobs".into(),
            "this node was built without backups (USE=backup)".into(),
        );
        let s = parse("backup_jobs: [{id: a, schedule: daily, targets: [x]}]").unwrap();
        let p = plan(&s, &live, false);
        assert!(
            p.errors.iter().any(|e| e.contains("USE=backup")),
            "{:?}",
            p.errors
        );
        // and export leaves it out
        assert!(!export(&live).extra.contains_key("backup_jobs"));
    }

    #[test]
    fn export_of_a_populated_node_is_a_fixed_point_for_every_kind() {
        let mut live = empty_node();
        live.kinds.users.insert(
            "alice".into(),
            UserPriv {
                id: "id-alice".into(),
                hash: HASH.into(),
            },
        );
        live.extra.insert(
            "users".into(),
            json!([{"username": "admin"}, {"username": "alice", "email": "a@x.org"}]),
        );
        live.extra.insert(
            "roles".into(),
            json!([{"name": "Operator", "privileges": ["VM.Audit", "VM.PowerMgmt"]}]),
        );
        live.extra.insert(
            "groups".into(),
            json!([{"name": "ops", "members": ["alice"]}]),
        );
        live.extra.insert(
            "acl".into(),
            json!([
                {"path": "/", "subject_type": "user", "subject": "admin", "role": "Administrator"},
                {"path": "/vms", "subject_type": "group", "subject": "ops", "role": "Operator"}
            ]),
        );
        live.extra.insert(
            "containers".into(),
            json!([{"name": "web", "runtime": "podman", "image": "docker.io/library/nginx:latest",
                    "memory": 256, "cpus": 1, "ports": ["8080:80"], "env": {"TZ": "UTC"},
                    "restart": "always", "power": "running"}]),
        );
        live.extra.insert(
            "volumes".into(),
            json!([{"name": "data", "runtime": "podman", "driver": "local"}]),
        );
        live.extra.insert(
            "backup_jobs".into(),
            json!([{"id": "n", "schedule": "0 2 * * *", "targets": ["web"]}]),
        );
        live.extra.insert(
            "ha_groups".into(),
            json!([{"name": "g", "nodes": ["n1"], "restricted": true}]),
        );
        live.extra.insert(
            "firewall".into(),
            json!({"options": {"policy_in": "accept", "policy_out": "accept"},
                   "aliases": [{"name": "o", "cidr": "192.0.2.0/24"}],
                   "ipsets": [], "security_groups": [],
                   "rules": [{"scope": "datacenter", "id": "ssh", "action": "ACCEPT",
                              "protocol": "tcp", "dport": "22"}]}),
        );
        let spec = export(&live);
        let yaml = serde_yaml::to_string(&spec).unwrap();
        let back = parse(&yaml).unwrap();
        assert_eq!(back, spec);
        let p = plan(&back, &live, true);
        assert!(p.in_sync(), "{:?} {:?}", actions(&p), p.errors);
    }
}
