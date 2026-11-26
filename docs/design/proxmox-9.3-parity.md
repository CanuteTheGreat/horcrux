# Proxmox VE 9.3 parity review

Status: review of `main` at 9d581e04 (2026-11-25) plus a live probe of the
published daily manager build. Each row was checked against the code and the
running API, not against docs or older comparison files.

Proxmox VE 9.3 shipped eight user-visible changes. This page records where
Horcrux stands on each one, what is missing, and the plan to close the gap.
Every item follows the project rules: nothing hard-coded (every value is a
typed key in the declarative config, the API and the UI), OpenRC and systemd
behave the same, standard ports, Incus/Podman preferred where a runtime is
involved, and features that need something the node lacks answer a
structured 424 with a reason instead of a 500.

## Summary

| # | Proxmox VE 9.3 feature | Horcrux today | Verdict |
|---|---|---|---|
| 1 | Installer: bond/LACP + VLAN-tagged management network | Installer management step takes one NIC with DHCP or a static address (`liveinst/app.py` `s_mgmt`); bonds are filtered out of the NIC list (`liveinst/system.py` `NIC_SKIP`); no VLAN field. Runtime bonds/VLANs exist after install (`crates/horcrux-cluster/src/cluster/nic.rs`, `crates/horcrux-sdn`). Now: management step and answer file take several NICs, bond mode (active-backup, 802.3ad with hash policy + LACP rate, balance-alb/tlb), miimon, VLAN id, DHCP/static; the live system builds the same link before the install; written as netifrc (OpenRC), systemd-networkd (systemd) or NetworkManager keyfiles (desktop) (`liveinst/netlayout.py`, `app.py`, `gui.py`, `consolemenu.py`, `tests/test_netlayout.py`); recorded in `/etc/horcrux/management-network.json`, shown and edited by `GET`/`PUT /api/network/management` (`horcrux-api/src/api/mgmt_net.rs`), page `/network/management` (`pages/mgmt_link.rs`) and declarative `management_network`; vmtest `tools/vmtest/scenarios/install_bond_vlan.py` (VLAN-only trunks, 802.3ad, HTTPS 443 on the VLAN, NIC failover) for both inits. | done |
| 2 | Node maintenance mode per node in the UI, evacuates HA resources, back on exit | `/api/nodes/maintenance` (+ `/plan`), drain planner (`crates/horcrux-cluster/src/cluster/drain.rs`), live-migration executor (`horcrux-api/src/api/drain.rs`), HA loop skips maintenance nodes (`api/ha_loop.rs`), UI page `/ha/maintenance` (`pages/ha/maintenance.rs`), rebalance `on_return`. Only for the node serving the UI ("this node"); no per-node control from the cluster view; on exit guests are spread by the rebalancer, not returned to the node they came from; no multi-node vmtest scenario. | partial |
| 3 | Notification matchers: nested AND/OR trees, drag-and-drop editor, calendar (weekdays + start/end), severity/type match | Channels exist (e-mail, Slack, Teams, Discord, PagerDuty, webhook; `crates/horcrux-monitoring/src/notification_channels.rs`), alert silences exist. `NotificationChannelStore::dispatch` sends every alert to every enabled channel: there is no matcher, no routing, no calendar, no event-type filter. Live: `/api/notifications/channels` only. | missing |
| 4 | Cluster-managed SAN multipath (iSCSI, FC, NVMe-oF), synced config, discovery wizard, per-node path view, LVM with snapshot volume chain, wipe/discard removed volumes | iSCSI pool with one portal (`crates/horcrux-storage/src/storage/iscsi.rs`: discovery, login, LUN listing), thick LVM with snapshots (`storage/lvm.rs`). No dm-multipath management, no NVMe-oF, no Fibre Channel, no multi-portal handling, no cluster-wide SAN config, no shared-LVM locking, no per-node path view, no wipe/discard on volume removal, no wizard. | partial (mostly missing) |
| 5 | System log viewer: structured, colour by priority, live mode, timespan, min priority, unit, syslog identifier with wildcard, kernel only, reboot separators | `/api/system/logs` (+ export, clear; `horcrux-api/src/api/system_admin/logs.rs`) reads journald or the syslog file; filters are level, service, search, limit; page `/system/logs`. No live follow, no time range, no unit vs identifier split, no wildcard, no kernel-only, no boot separators, no per-node choice in a cluster. | partial |
| 6 | API access log: who/where/what failed, precise timespans, request detail, instant statistics by client/user/token/path group/status, click-through filters | Audit log of explicit security events (logins, permission changes; `crates/horcrux-auth/src/audit/`, `/api/audit/*`, page `/audit`). The per-request audit middleware (`audit/middleware.rs`) is not wired into the router. Live: 7 authenticated GETs left the audit log at 2 events (the logins). No per-request record, no token/method/status/duration fields, no statistics. | missing |
| 7 | Third-party storage plugins configurable in the UI, form generated from a backend-declared schema, plugin SDK + docs + API changelog | Storage types are a closed enum (`StorageType` in `crates/horcrux-storage/src/storage/mod.rs`); the pool form is hand-written per type (`pages/storage/pool_management.rs`). No plugin interface, no schema, no SDK. | missing |
| 8 | Ceph release selection at install, opt-in newer kernel line, current QEMU / containers / OpenZFS | QEMU, Incus and OpenZFS track the newest Gentoo versions (`gentoo/package.accept_keywords/horcrux` accepts `~arch`). Ceph is read and managed (`api/ceph_admin.rs`) but not installed by Horcrux: it is commented out of `gentoo/sets/horcrux` and there is no install path or release choice. The installer offers no kernel line choice. | partial |

Gaps: 7 (3 missing, 4 partial); item 1 is done.

## Plan

Order of work, highest value first: 2, 4, 6, 5, 1, 3, 7, 8. Each item is its
own branch and lands on `main` only after CI is green.

### 2. Node maintenance from the cluster view

- `POST /api/nodes/:node/maintenance` and `GET /api/nodes/:node/maintenance`
  for any member; the serving node forwards to the target over the existing
  peer channel. The current local routes stay as aliases.
- Record each guest's origin node when a drain moves it. Exiting maintenance
  offers `return = origin | rebalance | none` (default `origin`, a typed key),
  so guests go back where they came from.
- Evacuate every HA resource on the node (VMs and containers, running and
  stopped-but-HA-managed), keep the plan/preview, and refuse with the reason
  when no member can take a guest.
- Cluster > Nodes list: maintenance badge and Enter/Exit actions per node,
  linked to the plan.
- Declarative: `nodes.<name>.maintenance` (state and return policy).
- Proof: vmtest 3-node scenario (drain node B, guests on A/C, HA does not
  place on B, exit returns them to B), OpenRC and systemd ISOs.

### 4. SAN: multipath for iSCSI, Fibre Channel and NVMe over Fabrics

- A `san` module in `horcrux-storage`: targets (iSCSI portal list, NVMe-oF
  TCP/RDMA subsystem NQN + addresses, FC by WWPN), discovery of every
  advertised portal, logins on every path, dm-multipath configuration
  generated from typed settings (no hand-edited `multipath.conf`), native
  NVMe multipath where the kernel uses it.
- Cluster-wide SAN config replicated to all members and applied on each.
- Per-node view: for each LUN, the block devices, paths and their state.
- Shared LVM on a LUN with node locking, snapshots as a qcow2 volume chain
  on thick LVM, `wipe_removed` and `discard_removed` per storage.
- UI wizard: portal -> discovered portals/paths -> LUN -> LVM -> review.
- 424 reasons when `multipathd`, `iscsiadm` or `nvme` are missing.
- Proof: vmtest scenario with a LIO target (targetcli) on 4 portals and an
  nvmet TCP target, 2 Horcrux nodes, path failure (link down) keeps I/O
  running; FC through emulation where possible, otherwise unit-tested
  config generation plus a stated limit.

### 6. API access log

- Wire a per-request access-log layer: time, node, user, token id (never the
  secret), realm, client address (peer; forwarded header only from a
  configured trusted proxy list), method, path, path group, status,
  duration, bytes, user agent.
- Bounded retention (typed keys: days and size), rotation, cluster query.
- `GET /api/access-log` (filters: time range with exact start/end, user,
  token, client, method, path substring, status class) and
  `GET /api/access-log/stats` (counts over time and by client, user, token,
  path group, status).
- UI page: filter bar, request detail, statistics panel whose values are
  links back to the filtered list.
- Proof: integration tests, RBAC route scan, Playwright page check.

### 5. System log viewer

- Journald: `journalctl -o json` with `--since/--until`, `-p`, `-u`,
  `-t` (wildcards matched in the API), `-k`, `--list-boots` for separators,
  and a follow stream over WebSocket/SSE.
- OpenRC hosts with syslog-ng/metalog: the same filters on the syslog file
  (identifier, priority from the facility/level when logged), boot
  separators from `wtmp` / kernel boot lines, follow by tailing.
- Per-node selection in a cluster, priority colours, live mode toggle.
- Proof: unit tests for both sources, OpenRC + systemd ISO checks, ux crawl.

### 1. Installer: bonds and VLAN-tagged management

- Management step: interfaces (one or several), bond mode (active-backup,
  802.3ad LACP with hash policy, balance-alb), optional VLAN id, address mode.
- Same keys in the answer file; written for both init systems (netifrc and
  systemd-networkd), and the management bridge sits on top of bond/VLAN.
- Both front ends (text and graphical) and the console menu.
- Proof: vmtest scenario with two NICs into a hub that only carries tagged
  traffic: the installed node answers HTTPS on the VLAN at first boot;
  OpenRC and systemd.

### 3. Notification matchers

- Matchers as typed trees: `all` / `any` / `not` groups with leaves on
  severity, event type (alert rule, backup, update, HA, replication...),
  node, guest, calendar (weekdays + start/end time, timezone) and field
  regex; each matcher names its targets; an implicit default matcher keeps
  today's behaviour.
- Events that are not alerts (backup jobs, package updates, HA events) go
  through the same router.
- UI tree editor with drag-and-drop between groups and a test button that
  evaluates a sample event.
- Proof: unit tests of the evaluator, API tests, Playwright drag-and-drop.

### 7. Storage plugins

- Out-of-process plugin interface (an executable speaking JSON over stdio)
  that declares its schema: fields, types, hints, limits, secrets; plus the
  volume operations (create, delete, list, resize, snapshot, activate).
- The API validates pool config against the declared schema; the UI renders
  the form from it.
- SDK: a Rust crate and a reference plugin (SSHFS), development guide and a
  versioned storage-API changelog.
- Proof: plugin installed as a package, pool created in the UI, VM disk on it.

### 8. Ceph release and kernel line

- Ceph install from the UI/CLI/declarative config with the release chosen
  from the versions the package repository provides (overlay ebuild for the
  current upstream release when `::gentoo` lags), plus upgrade path.
- Installer and API choice of kernel line: default (current stable) or the
  newest kernel as an opt-in, recorded as a typed key and kept through
  updates and boot environments.
- Proof: ISO checks on both init systems; Ceph 3-node vmtest.
