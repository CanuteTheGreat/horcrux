# Feature tests

Every user-facing Horcrux feature, the end-to-end test that drives the REAL product (API / UI, real guests and containers, results checked from inside the guest or system; no mocks), where it runs, and its last result.
Generated from the lab's feature matrix (`scripts/lab/vmcluster/matrix.py` in the hermes lab repo) and the daily/on-change VM-cluster runs; edit the matrix, not this file.

**Where tests run.** `VM` = in a 3-node cluster of Horcrux VMs on the kvm runners (nested KVM, REAL manager, REAL guests), on every relevant change and the full suite daily. `HW` = needs the physical machine (Wake-on-LAN, power loss, hardware watchdog, real NIC/Wi-Fi, suspend, GPU/PCI passthrough, real-disk ZFS/SMART, firmware/T2); weekly hardware-validation batch on the 3 lab laptops.

**Test column.** `vmc:` = the VM-cluster suite; `ci:` = an e2e that exists and runs in CI containers (to be ported into the VM cluster); `rust:` = `horcrux-api/tests/it` suite against a live server; `lab:` = lab-laptop job; `-` = **no e2e test yet**.

**Coverage (2026-11-07).** 64 features claimed. Passing e2e in the VM cluster: **21**; failing: **3**; written, not yet run: 0; e2e exists in CI only: 31; hardware-only: 4; no e2e test: **5**.

A failing feature, or a claimed-but-missing one, is a bug: it is filed in qa-findings/horcrux.md and fixed (or the claim is corrected here and in the docs).

| id | area | feature | claimed in | test | mode | last result |
|---|---|---|---|---|---|---|
| `vm.create-boot` | VMs | create a VM, import a disk, boot a real guest OS (nested KVM), see it in the guest | README Virtualization; API vms | vmc: nodetests/vm_lifecycle.py | VM | pass, 2026-11-07 (VM cluster run 1107-2210) |
| `vm.console` | VMs | serial console reaches the guest | README Console access | vmc: nodetests/vm_lifecycle.py | VM | pass, 2026-11-07 (VM cluster run 1107-2210) |
| `vm.guest-data` | VMs | guest disk persists data | - | vmc: nodetests/vm_lifecycle.py | VM | pass, 2026-11-07 (VM cluster run 1107-2210) |
| `vm.snapshot-rollback` | VMs | snapshot and roll back (data written after the snapshot is gone) | README, docs/API.md snapshots | vmc: nodetests/vm_lifecycle.py | VM | pass, 2026-11-07 (VM cluster run 1107-2210) |
| `vm.clone-running` | VMs | full clone of a running VM | README Templates: full and linked clones | vmc: nodetests/vm_lifecycle.py | VM | **FAIL** 2026-11-07: clone of a RUNNING VM (500 {'status': 500, 'error': 'INTERNAL_ERROR', 'message': 'An internal server error occurred', 'details': 'Failed to clone VM:  |
| `vm.clone` | VMs | full clone (stopped source) boots with the source's data | README Templates | vmc: nodetests/vm_lifecycle.py | VM | pass, 2026-11-07 (VM cluster run 1107-2210) |
| `vm.disk-add` | VMs | add a disk, guest sees it | UI VM hardware | vmc: nodetests/vm_lifecycle.py | VM | pass, 2026-11-07 (VM cluster run 1107-2210) |
| `vm.delete` | VMs | delete VM removes QEMU, files and record | API vms | vmc: nodetests/vm_lifecycle.py | VM | pass, 2026-11-07 (VM cluster run 1107-2210) |
| `vm.nic-bridge-vlan-pxe` | VMs | NIC on a VLAN-aware bridge, PXE DHCP seen on the right VLAN, CD insert/eject, shutdown/reboot/reset | docs/API.md | ci: tests/vm_nic_e2e.py | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `vm.devices-balloon-agent` | VMs | balloon, guest agent, hotplug | API vms | ci: tests/vm_devices_e2e.py | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `vm.advanced-serial-virtiofs` | VMs | extra serial ports, virtiofs, hookscript | UI vm_advanced | ci: tests/vm_advanced_e2e.py | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `vm.save-state` | VMs | save state on host shutdown / resume | docs/SAVE_STATE.md | ci: tests/guest_state_e2e.py, guest-power.yml | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `vm.template-cloudinit` | VMs | templates + cloud-init | README Templates, API cloudinit | ui: template_checks (no cloud-init guest boot test) | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `vm.import-ovf-esxi` | VMs | OVF / ESXi import | docs/OVF_IMPORT.md | rust: ovf_import_tests, esxi_import_tests (mock ESXi) | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `vm.display-vnc-spice` | VMs | VNC (noVNC proxy) / SPICE display | README Console access | rust: console_tests; ci: incus-vm-console.yml | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `vm.gpu-pci-passthrough` | VMs | GPU / PCI / USB passthrough | docs/GPU_PASSTHROUGH.md | - | HW | no end-to-end test exists (gap) |
| `ct.run` | Containers | Docker container: create, start, really running in the runtime | README Container runtimes | vmc: nodetests/ct_stacks.py | VM | pass, 2026-11-07 (VM cluster run 1107-2210) |
| `ct.exec` | Containers | exec runs inside the container | API containers | vmc: nodetests/ct_stacks.py | VM | pass, 2026-11-07 (VM cluster run 1107-2210) |
| `ct.logs` | Containers | log stream (ticket + WebSocket) | API containers | vmc: nodetests/ct_stacks.py (ticket + runtime logs; WS payload not read) | VM | pass, 2026-11-07 (VM cluster run 1107-2210) |
| `ct.stop-start` | Containers | stop / start | API containers | vmc: nodetests/ct_stacks.py | VM | pass, 2026-11-07 (VM cluster run 1107-2210) |
| `ct.delete` | Containers | delete removes it from the runtime | API containers | vmc: nodetests/ct_stacks.py | VM | pass, 2026-11-07 (VM cluster run 1107-2210) |
| `stack.deploy` | Containers | compose stack deploy, published port answers real HTTP | API stacks | vmc: nodetests/ct_stacks.py | VM | pass, 2026-11-07 (VM cluster run 1107-2210) |
| `stack.stop-start` | Containers | stack stop / start (port closes / opens) | API stacks | vmc: nodetests/ct_stacks.py | VM | pass, 2026-11-07 (VM cluster run 1107-2210) |
| `stack.update` | Containers | update a deployed stack to a new image | owner list | vmc: nodetests/ct_stacks.py | VM | **FAIL** 2026-11-07: update of a deployed stack by POST /stacks refused (409): no update path through the API |
| `stack.remove` | Containers | stack remove | API stacks | vmc: nodetests/ct_stacks.py | VM | pass, 2026-11-07 (VM cluster run 1107-2210) |
| `ct.lxc` | Containers | LXC: templates, create, console, limits, checkpoint | docs/LXC.md | ci: tests/lxc_e2e.py | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `ct.incus` | Containers | Incus / LXD containers and VMs, consoles | README | ci: tests/incus_e2e.py | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `ct.podman` | Containers | Podman containers | README | ci: drain_ct_vm_e2e.py (Podman leg) | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `ct.save-state` | Containers | container checkpoint/restore (CRIU) | docs/SAVE_STATE.md | ci: tests/guest_state_ct_e2e.py | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `net.bridge-create` | Networking | Host Bridges: create VLAN-aware bridge with an uplink port through the API | docs/NEW_FEATURES.md VLAN | vmc: nettests.py | VM | pass, 2026-11-07 (VM cluster run 1107-2210) |
| `net.vlan-access-ports` | Networking | VM NIC with vlan N = access port with PVID N on the bridge | docs/NEW_FEATURES.md VLAN | vmc: nettests.py | VM | pass, 2026-11-07 (VM cluster run 1107-2210) |
| `net.cross-node-guest-l2` | Networking | guests on different nodes talk on the same VLAN | docs/NEW_FEATURES.md VLAN/VXLAN | vmc: nettests.py | VM | pass, 2026-11-07 (VM cluster run 1107-2210) |
| `net.vlan-isolation` | Networking | guests on different VLANs cannot talk | VLAN | vmc: nettests.py | VM | pass, 2026-11-07 (VM cluster run 1107-2210) |
| `fw.vm-icmp-block-allow` | Networking | VM firewall rule really drops / re-allows pings between guests | docs/API.md firewall | vmc: nettests.py | VM | pass, 2026-11-07 (VM cluster run 1107-2210) |
| `fw.vm-tcp-port-block-allow` | Networking | VM firewall rule blocks / allows a TCP port (real connection from another guest) | docs/API.md firewall | vmc: nettests.py | VM | pass, 2026-11-07 (VM cluster run 1107-2210) |
| `net.guest-dhcp-dns` | Networking | DHCP / DNS for guests on a bridge or vnet | owner request (README mentions none) | vmc: nettests.py (records the gap) | VM | **FAIL** 2026-11-07: Horcrux offers no DHCP/DNS service for guests (sdn_zones.rs: 'Horcrux runs no DHCP server'); static IPAM registry only |
| `fw.datacenter-nft-apply` | Networking | datacenter firewall rules / ipsets / aliases land in the kernel nftables | docs/API.md firewall | ci: tests/firewall_nft_e2e.sh | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `net.sdn-zones-ipam` | Networking | SDN zones/vnets/subnets, static IPAM | docs/NEW_FEATURES.md | rust: sdn_tests; ci: sdn_cni_e2e.sh | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `net.cni-policies` | Networking | CNI attach + network policies enforced | README real CNI/nftables | ci: tests/sdn_cni_e2e.py | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `net.ovs-vxlan` | Networking | Open vSwitch + VXLAN networks | docs/NEW_FEATURES.md | - | VM | no end-to-end test exists (gap) |
| `ha.crash-failover` | HA | guest back on a survivor <= 120 s after a node crash, no data loss | ROADMAP, README | lab: scenarios/hasuite.py s1/s1b (HW laptops today); ci: ha_failover_e2e.py | VM+HW | e2e exists in CI containers, not yet moved into the VM cluster |
| `ha.partition-no-double-run` | HA | partition of 1 / 2 nodes: never two copies of a guest | lab G1 | lab: hasuite s3/s4 (HW laptops today) | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `ha.manager-kill-hang` | HA | manager killed / hung: restarted, guest survives | lab G1 | lab: hasuite | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `ha.quorum-loss` | HA | quorum loss handling | lab G1 | lab: hasuite | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `ha.power-off-wol` | HA | power-off + Wake-on-LAN recovery | lab G1 | lab: hasuite s2 (skipped: WoL unproven) | HW | hardware batch only (see lab PLAN.md for the latest laptop result) |
| `ha.drain-rebalance` | HA | drain a node / placement rules | docs | ci: tests/drain_e2e.py, drain_ct_vm_e2e.py | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `ha.node-replacement` | HA | replace a node, cluster re-learns it | lab G1/G5 | lab: hasuite / cluster-replace | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `mig.live-vm` | HA | live migration of a running guest keeps serving traffic | docs/MIGRATION.md | ci: vm_migrate_e2e.py (2 container nodes); lab: cluster-migrate | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `mig.rolling-upgrade` | HA | rolling upgrade with guests running | lab G5 | lab: cluster-upgrade, hasuite | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `sto.zfs-datasets` | Storage | ZFS pools/datasets/snapshots via the API | docs/NEW_FEATURES.md ZFS | lab: hx-zfs (HW laptops: real disks) | HW | hardware batch only (see lab PLAN.md for the latest laptop result) |
| `sto.zfs-encryption` | Storage | ZFS encryption | README | - | VM | no end-to-end test exists (gap) |
| `sto.volumes-content` | Storage | storage pools, volumes, content | API storage | rust: storage_volume_tests, storage_content_tests | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `sto.nfs-iscsi-ceph` | Storage | NFS / iSCSI / Ceph storage backends | docs | - | VM | no end-to-end test exists (gap) |
| `sto.replication` | Storage | replication jobs | API replication | rust: replication_api_tests (no second node) | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `bak.backup-restore` | Storage | backup a VM and restore it, data identical in the guest | API backups | rust: backup_api_e2e, backup_tests (no guest check) | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `sto.snapshot-schedules` | Storage | snapshot schedules and quotas | API snapshot-schedules | - | VM | no end-to-end test exists (gap) |
| `auth.users-rbac` | Auth | users, groups, roles, ACLs: RBAC enforced on every route | docs/RBAC.md | rust: rbac_matrix_tests, auth_e2e_tests | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `auth.tokens-totp` | Auth | API tokens, TOTP, sessions | docs/ACCESS_CONTROL.md | ci: ui_e2e.py | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `auth.realms` | Auth | LDAP / OIDC / PAM realms | README | ci: tests/auth_realms_e2e.sh (real OpenLDAP, Dex) | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `ui.flows` | UI | web UI flows (login, VMs, containers, storage, firewall, users, monitoring...) | UI pages | ci: horcrux-ui/e2e/ui_e2e.py (Playwright, 1767 lines) | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `mon.metrics-alerts` | Ops | metrics, metric servers, alerts, webhooks | docs/METRICS.md, ALERT_NOTIFICATIONS_GUIDE.md | rust: alerts_tests, metric_servers_tests, webhook_events_tests; ui monitoring | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `sys.updates-binhost` | Ops | update the installed system from the binhost, reboot, BE rollback | ROADMAP criterion 2 | lab: upgrade-test, be-rollback-test (HW laptops) | HW | hardware batch only (see lab PLAN.md for the latest laptop result) |
| `k8s.clusters` | Ops | Kubernetes clusters / Helm | docs/KUBERNETES.md | rust: k8s_restart_tests; ci: k8s_helm_setup.sh | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `hw.suspend-wdt-wifi` | Hardware | watchdog, suspend, Wi-Fi, NIC drivers, T2/firmware | lab G9 | lab: hw-matrix, wifi-reconnect, hx-hw-features (laptops) | HW | hardware batch only (see lab PLAN.md for the latest laptop result) |

## Failures found by the VM cluster

- `vm.clone-running` (full clone of a running VM): **FAIL** 2026-11-07: clone of a RUNNING VM (500 {'status': 500, 'error': 'INTERNAL_ERROR', 'message': 'An internal server error occurred', 'details': 'Failed to clone VM: 
- `stack.update` (update a deployed stack to a new image): **FAIL** 2026-11-07: update of a deployed stack by POST /stacks refused (409): no update path through the API
- `net.guest-dhcp-dns` (DHCP / DNS for guests on a bridge or vnet): **FAIL** 2026-11-07: Horcrux offers no DHCP/DNS service for guests (sdn_zones.rs: 'Horcrux runs no DHCP server'); static IPAM registry only
