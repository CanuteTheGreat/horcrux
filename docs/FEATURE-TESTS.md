# Feature tests

Every user-facing Horcrux feature, the end-to-end test that drives the REAL product (API / UI, real guests and containers, results checked from inside the guest or system; no mocks), where it runs, and its last result.
Generated from the lab's feature matrix (`scripts/lab/vmcluster/matrix.py` in the hermes lab repo) and the daily/on-change VM-cluster runs; edit the matrix, not this file.

**Where tests run.** `VM` = in a 3-node cluster of Horcrux VMs on the kvm runners (nested KVM, REAL manager, REAL guests), on every relevant change and the full suite daily. `HW` = needs the physical machine (Wake-on-LAN, power loss, hardware watchdog, real NIC/Wi-Fi, suspend, GPU/PCI passthrough, real-disk ZFS/SMART, firmware/T2); weekly hardware-validation batch on the 3 lab laptops.

**VM-cluster suites** (`scripts/lab/vmcluster/`; one run = 3 Horcrux node VMs on the kvm runner running the all-features manager build): `vm` `ct` `net` (guests, containers, bridges/VLANs/firewall), `ci` (the repo's own CI-container e2e scripts, run ON a node VM), `live` (live migration of a running guest while ping and TCP traffic flows from a third node), `storage` (backup/restore with a guest data check; ZFS datasets, encryption, replication), `ui` (the Playwright suite in a real browser) and `ha` (hard crash, partitions, manager kill/hang, quorum loss, node replacement, NFS loss, local-disk guest: the lab's `hasuite.py` unchanged; goal G1 = 10 green runs in a row).

**Test column.** `vmc:` = the VM-cluster suite; `ci:` = an e2e that exists and runs in CI containers (to be ported into the VM cluster); `rust:` = `horcrux-api/tests/it` suite against a live server; `lab:` = lab-laptop job; `-` = **no e2e test yet**.

**Coverage (2026-11-16).** 77 features claimed. Passing e2e in the VM cluster: **56**; failing: **0**; written, not yet run: 3; e2e exists in CI only: 13; hardware-only: 4; no e2e test: **1**.

A failing feature, or a claimed-but-missing one, is a bug: it is filed in qa-findings/horcrux.md and fixed (or the claim is corrected here and in the docs).

| id | area | feature | claimed in | test | mode | last result |
|---|---|---|---|---|---|---|
| `vm.create-boot` | VMs | create a VM, import a disk, boot a real guest OS (nested KVM), see it in the guest | README Virtualization; API vms | vmc: nodetests/vm_lifecycle.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `vm.console` | VMs | serial console reaches the guest | README Console access | vmc: nodetests/vm_lifecycle.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `vm.guest-data` | VMs | guest disk persists data | - | vmc: nodetests/vm_lifecycle.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `vm.snapshot-rollback` | VMs | snapshot and roll back (data written after the snapshot is gone) | README, docs/API.md snapshots | vmc: nodetests/vm_lifecycle.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `vm.clone-running` | VMs | full clone of a running VM | README Templates: full and linked clones | vmc: nodetests/vm_lifecycle.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `vm.clone-running-live` | VMs | live full clone of a running VM: guest never paused, clone boots with the source's data, linked clone refused (409) | docs/API.md Clone VM | vmc: nodetests/fix_node.py | VM | written, not yet run |
| `vm.clone` | VMs | full clone (stopped source) boots with the source's data | README Templates | vmc: nodetests/vm_lifecycle.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `vm.disk-add` | VMs | add a disk, guest sees it | UI VM hardware | vmc: nodetests/vm_lifecycle.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `vm.delete` | VMs | delete VM removes QEMU, files and record | API vms | vmc: nodetests/vm_lifecycle.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `vm.nic-bridge-vlan-pxe` | VMs | NIC on a VLAN-aware bridge, PXE DHCP seen on the right VLAN, CD insert/eject, shutdown/reboot/reset | docs/API.md | vmc: ciport.py running the CI e2e vm_nic_e2e.py on a VM node | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `vm.devices-balloon-agent` | VMs | balloon, guest agent, hotplug | API vms | vmc: ciport.py running the CI e2e vm_devices_e2e.py on a VM node | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `vm.advanced-serial-virtiofs` | VMs | extra serial ports, virtiofs, hookscript | UI vm_advanced | vmc: ciport.py running the CI e2e vm_advanced_e2e.py on a VM node | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `vm.save-state` | VMs | save state on host shutdown / resume | docs/SAVE_STATE.md | ci: tests/guest_state_e2e.py, guest-power.yml | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `vm.template-cloudinit` | VMs | templates + cloud-init | README Templates, API cloudinit | vmc: nodetests/cloudinit_guest.py (seed applied by a real CirrOS guest; templates: ui template_checks) | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `vm.template-cloudinit-delete` | VMs | DELETE /api/cloudinit/{vm} removes the seed; the VM then boots without the CD-ROM | API cloudinit | vmc: nodetests/cloudinit_guest.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `vm.import-ovf-esxi` | VMs | OVF / ESXi import | docs/OVF_IMPORT.md | rust: ovf_import_tests, esxi_import_tests (mock ESXi) | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `vm.display-vnc` | VMs | VNC console: the ticketed WebSocket proxy relays QEMU's RFB session, a wrong password is refused, the framebuffer shows the guest boot | README Console access | vmc: nodetests/display_guest.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `vm.display-spice` | VMs | SPICE console: password-protected SPICE server answers a link request; .vv file is served | README Console access | vmc: nodetests/display_guest.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `vm.gpu-pci-passthrough` | VMs | GPU / PCI / USB passthrough | docs/GPU_PASSTHROUGH.md | - | HW | no end-to-end test exists (gap) |
| `ct.run` | Containers | Docker container: create, start, really running in the runtime | README Container runtimes | vmc: nodetests/ct_stacks.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `ct.exec` | Containers | exec runs inside the container | API containers | vmc: nodetests/ct_stacks.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `ct.logs` | Containers | log stream (ticket + WebSocket) | API containers | vmc: nodetests/ct_stacks.py (ticket + runtime logs; WS payload not read) | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `ct.stop-start` | Containers | stop / start | API containers | vmc: nodetests/ct_stacks.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `ct.delete` | Containers | delete removes it from the runtime | API containers | vmc: nodetests/ct_stacks.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `stack.deploy` | Containers | compose stack deploy, published port answers real HTTP | API stacks | vmc: nodetests/ct_stacks.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `stack.stop-start` | Containers | stack stop / start (port closes / opens) | API stacks | vmc: nodetests/ct_stacks.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `stack.update` | Containers | update a deployed stack to a new image | owner list | vmc: nodetests/ct_stacks.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `stack.update-rolling` | Containers | PUT /api/stacks/:id: rolling update keeps volumes + unchanged services, env merge, bad image refused/rolled back, stopped stack started | docs/API.md Stacks | vmc: nodetests/fix_node.py | VM | written, not yet run |
| `stack.remove` | Containers | stack remove | API stacks | vmc: nodetests/ct_stacks.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `ct.lxc` | Containers | LXC: templates, create, console, limits, checkpoint | docs/LXC.md | vmc: ciport.py running the CI e2e lxc_e2e.py on a VM node | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `ct.incus` | Containers | Incus / LXD containers: create, console, exec, limits, snapshots (incus_e2e.py MODE=ct) | README | vmc: ciport.py running the CI e2e incus_e2e.py (MODE=ct) on a VM node | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `ct.incus-vm` | Containers | Incus VMs: create, SPICE/VNC console handshake, .vv file (incus_e2e.py MODE=vm) | README | vmc: ciport.py running the CI e2e incus_e2e.py (MODE=vm) on a VM node | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `ct.podman` | Containers | Podman containers | README | vmc: ciport.py running the CI e2e guest_state_ct_vm_e2e.py (podman leg) on a VM node | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `ct.save-state` | Containers | container checkpoint/restore (CRIU) | docs/SAVE_STATE.md | vmc: ciport.py running the CI e2e guest_state_ct_vm_e2e.py on a VM node | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `net.bridge-create` | Networking | Host Bridges: create VLAN-aware bridge with an uplink port through the API | docs/NEW_FEATURES.md VLAN | vmc: nettests.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `net.vlan-access-ports` | Networking | VM NIC with vlan N = access port with PVID N on the bridge | docs/NEW_FEATURES.md VLAN | vmc: nettests.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `net.cross-node-guest-l2` | Networking | guests on different nodes talk on the same VLAN | docs/NEW_FEATURES.md VLAN/VXLAN | vmc: nettests.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `net.vlan-isolation` | Networking | guests on different VLANs cannot talk | VLAN | vmc: nettests.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `fw.vm-icmp-block-allow` | Networking | VM firewall rule really drops / re-allows pings between guests | docs/API.md firewall | vmc: nettests.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `fw.vm-tcp-port-block-allow` | Networking | VM firewall rule blocks / allows a TCP port (real connection from another guest) | docs/API.md firewall | vmc: nettests.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `net.trunk-vlans-api` | Networking | trunk VLANs on a bridge uplink port through the bridge API (POST/PUT trunk_vlans), live change, validation | docs/API.md Host bridges | vmc: nettests.py (fixtests.trunk) | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `net.guest-dhcp-dns` | Networking | DHCP / DNS for guests on a bridge or vnet | owner request (README mentions none) | vmc: nettests.py (fixtests.dhcp_dns: 2-node cluster, one dnsmasq, guests on both nodes) | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `fw.datacenter-nft-apply` | Networking | datacenter firewall rules / ipsets / aliases land in the kernel nftables | docs/API.md firewall | vmc: ciport.py running the CI e2e firewall_objects_e2e.py on a VM node | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `net.sdn-zones-ipam` | Networking | SDN zones/vnets/subnets, static IPAM | docs/NEW_FEATURES.md | vmc: ciport.py running the CI e2e sdn_cni_e2e.py on a VM node | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `net.cni-policies` | Networking | CNI attach + network policies enforced | README real CNI/nftables | vmc: ciport.py running the CI e2e sdn_cni_e2e.py on a VM node | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `net.ovs-vxlan` | Networking | Open vSwitch + VXLAN networks: bridge/VLAN port/VXLAN port/OpenFlow rule lifecycle checked with ovs-vsctl, ovs-ofctl and ip link; hostile names refused; 424 without OVS | docs/NEW_FEATURES.md | ci: tests/ovs_e2e.sh (real Open vSwitch in a privileged sibling; ci.yml 'Open vSwitch' step; pass in CI run 17675, 2026-11-16) | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `ha.crash-failover` | HA | guest back on a survivor <= 120 s after a node crash (sysrq-b, with and without the node coming back), no data loss in the checksummed guest workload | ROADMAP, README | vmc: hatests.py (scenarios/hasuite.py s1/s1b) | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `ha.partition-no-double-run` | HA | network partition of 1 node / of 2 nodes (no majority anywhere): the minority self-fences, never two copies of a guest | lab G1 | vmc: hatests.py (hasuite s3/s4) | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `ha.manager-kill-hang` | HA | manager killed / hung: restarted by the supervisor, the guest survives, no double run | lab G1 | vmc: hatests.py (hasuite s5/s6) | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `ha.quorum-loss` | HA | quorum loss (2 of 3 nodes crash): the lone survivor starts nothing; the guest returns when a majority is back | lab G1 | vmc: hatests.py (hasuite s7) | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `ha.power-off-wol` | HA | power-off + Wake-on-LAN recovery | lab G1 | lab: cluster-hasuite s2 on gringotts (3 laptops; RTC alarm armed first) | HW | hardware (3 laptops), 2026-11-10: forced power-off of gringotts: guest back on a survivor in 109-122 s (criterion 120 s; one run 121.9 s), no data loss, no double run; the node came back by the RTC wake alarm (~650 s). Wake-on-LAN itself does NOT wake it from S5 (BIOS) |
| `ha.drain-rebalance` | HA | drain a node / placement rules | docs | ci: tests/drain_e2e.py, drain_ct_vm_e2e.py | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `ha.node-replacement` | HA | replace a node (remove + re-join), cluster re-learns it, the running guest is never interrupted | lab G1/G5 | vmc: hatests.py (hasuite s10) | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `mig.live-vm` | HA | live migration of a running guest keeps serving traffic: ping 20 Hz + one long TCP session + fresh TCP connections from a third node across n1->n2->n1, downtime measured | docs/MIGRATION.md | vmc: livetests.py (local disks over NBD and NFS shared disks) | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `mig.live-local-disk` | HA | live migration with node-local disks (drive-mirror over NBD) under traffic | docs/MIGRATION.md | vmc: livetests.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `mig.live-shared-disk` | HA | live migration with disks on NFS shared storage (no disk copy) under traffic | docs/MIGRATION.md | vmc: livetests.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `mig.ha-guest` | HA | live migration of an HA-managed guest keeps its HA record, data intact | docs/MIGRATION.md, HA | vmc: hatests.py (hasuite s8) | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `ha.nfs-server-loss` | HA | NFS shared-storage server crashes: guests stall, no HA action, I/O resumes when it returns | lab G1 | vmc: hatests.py (hasuite s11) | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `ha.local-disk-guest` | HA | guest with a node-local disk is NOT restarted elsewhere with a stale disk when its node dies; it restarts when the node returns | lab G1 | vmc: hatests.py (hasuite s12) | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `mig.rolling-upgrade` | HA | rolling upgrade with guests running (needs the Gentoo emerge path) | lab G5 | lab: hasuite s9, cluster-upgrade (HW laptops) | HW | hardware batch only (see lab PLAN.md for the latest laptop result) |
| `sto.zfs-datasets` | Storage | ZFS pool registered as a Horcrux pool, zvol volumes create/list/resize/delete, a VM running on a ZFS dataset (directory pool), VM snapshot | docs/NEW_FEATURES.md ZFS | vmc: storetests.py (file-vdev pool in the node VM); lab: hx-zfs (real disks) | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `sto.zfs-encryption` | Storage | ZFS native encryption: encrypted dataset as VM storage (lock = VM cannot run, unlock = data intact); the encrypted-root install is covered by the installer VM tests | docs/ISO-INSTALL.md (encrypted ZFS root) | vmc: storetests.py; lab: hx-installer-vm zfs-encrypted | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `sto.volumes-content` | Storage | storage pools, volumes, content | API storage | rust: storage_volume_tests, storage_content_tests | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `sto.nfs-pool` | Storage | NFS storage pool: add by server:/export against nfs-ganesha, volumes on the mount | docs | ci: tests/nfs_storage_e2e.sh (ci.yml 'NFS storage pool' step; pass in CI run 17675, 2026-11-16) | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `sto.iscsi-pool` | Storage | iSCSI storage pool: add by iscsi://portal/iqn against a tgt target, login/logout with the pool | docs | ci: tests/iscsi_storage_e2e.sh (ci.yml 'iSCSI storage pool' step; pass in CI run 17675, 2026-11-16) | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `sto.ceph-rbd-pool` | Storage | Ceph RBD pool against a real single-node Ceph cluster: real capacity, RBD image create/resize/delete, duplicate 409, over-size 400, shrink 400 | docs | ci: tests/ceph_storage_e2e.sh (ci.yml 'Ceph storage pool' step; pass in CI run 17675, 2026-11-16) | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `sto.replication` | Storage | replication job: VM snapshot -> zfs send|receive to a second node, data identical | API replication | vmc: storetests.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `bak.backup-restore` | Storage | backup a RUNNING VM (zstd and dedup), verify, delete it, restore over it and to a new id: data written in the guest is identical | API backups | vmc: storetests.py | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `sto.snapshot-schedules` | Storage | snapshot schedules: cron schedule snapshots by itself, retention prunes only its own, disable/delete stop it (quotas: no test yet) | API snapshot-schedules | ci: tests/snapshot_schedule_e2e.sh (real qcow2) | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `auth.users-rbac` | Auth | users, groups, roles, ACLs: RBAC enforced on every route | docs/RBAC.md | rust: rbac_matrix_tests, auth_e2e_tests | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `auth.tokens-totp` | Auth | API tokens, TOTP, security keys, sessions | docs/ACCESS_CONTROL.md | vmc: uitests.py (ui_e2e.py auth part) | VM | written, not yet run |
| `auth.realms` | Auth | LDAP / OIDC / PAM realms | README | ci: tests/auth_realms_e2e.sh (real OpenLDAP, Dex) | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `ui.flows` | UI | web UI flows (login, 2FA, containers, stacks, events, notifications, metrics, realms, certificates, themes...) in a real browser against a VM node | UI pages | vmc: uitests.py running horcrux-ui/e2e/ui_e2e.py (Playwright) | VM | pass, 2026-11-16 (VM cluster run 1116-1039) |
| `mon.metrics-alerts` | Ops | metrics, metric servers, alerts, webhooks | docs/METRICS.md, ALERT_NOTIFICATIONS_GUIDE.md | rust: alerts_tests, metric_servers_tests, webhook_events_tests; ui monitoring | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `sys.updates-binhost` | Ops | update the installed system from the binhost, reboot, BE rollback | ROADMAP criterion 2 | lab: upgrade-test, be-rollback-test (HW laptops) | HW | hardware batch only (see lab PLAN.md for the latest laptop result) |
| `k8s.clusters` | Ops | Kubernetes clusters / Helm | docs/KUBERNETES.md | rust: k8s_restart_tests; ci: k8s_helm_setup.sh | VM | e2e exists in CI containers, not yet moved into the VM cluster |
| `hw.suspend-wdt-wifi` | Hardware | watchdog, suspend, Wi-Fi, NIC drivers, T2/firmware | lab G9 | lab: hw-matrix, wifi-reconnect, hx-hw-features (laptops) | HW | hardware batch only (see lab PLAN.md for the latest laptop result) |

## G1 streak (HA suite in the 3-VM cluster)

22 consecutive green `vmc-hasuite` runs (goal G1 needs 10, plus one confirmation run on the 3 real laptops).

## Measured in the last full VM run (vmc-1116-1039.log)

HA scenarios (guest = CirrOS with a checksummed 1 Hz write workload; outage = gap in the guest's records):

| scenario | detected offline after (s) | guest running again after (s) | guest outage (s) | data integrity | seconds with two copies | human needed |
|---|---|---|---|---|---|---|
| hard crash | 8.0 | 36.0 | 40 | OK | 0 | no |
| hard crash, node stays down | 8.0 | 33.1 | 40 | OK | 0 | no |
| hard crash, node returns late | 8.0 | 34.1 | 39 | OK | 0 | no |
| partition 1 | 8.0 | 36.0 | 41 | OK | 0 | no |
| partition 2 | None | 5.5 | 179 | OK | 0 | no |
| manager killed | None | None | 10 | OK | 0 | no |
| manager hung | 8.0 | 18.0 | 6 | OK | 0 | no |
| quorum loss | None | 126.0 | 41 | OK | 0 | no |
| live migration | None | None | None | OK | - | no |
| node replacement | None | 13.7 | None | OK | - | no |
| NFS server loss | None | None | 33 | OK | - | no |
| local disk | None | 161.5 | 41 | OK | - | no |

Live migration under traffic (ping 20 Hz + TCP session + fresh connections from a third node; each line = one migration):

| downtime = longest ICMP gap (s) | lost pings | migration time (s) | TCP stream longest stall (s) | fresh TCP connections failed / total |
|---|---|---|---|---|
| 0.16 | 2 | 2.7 | 1.11 | 1 / 107 |
| 0.16 | 2 | 2.7 | 1.1 | 0 / 109 |
| 0.16 | 2 | 2.7 | 1.1 | 10 / 118 |
| 0.16 | 2 | 2.7 | 1.1 | 0 / 109 |

## Failures found by the VM cluster


