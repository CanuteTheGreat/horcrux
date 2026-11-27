# Horcrux vs Proxmox VE: functional and workflow parity

Status: review of `main` plus the ui-kit v0.3 branch (2026-11-27), done three ways: the code (file references below),
a live dogfood run of the daily manager build against a throwaway node (every nav route, then per-object pages for a
running VM, a stopped VM, and a VM/container that does not exist), and a comparison with how Proxmox VE 9.x handles
the same task. It complements `proxmox-9.3-parity.md`, which covers the 9.3 release features; this page covers the
everyday flows a Proxmox admin expects to just work.

Why this page exists: earlier UI rounds compared Horcrux with Proxmox on looks (grid, type scale, toolbar density) and
checked pages one at a time. The owner then found, in minutes, that the console of a stopped VM said "start it" with
no Start button, spun "Connecting..." forever and printed the raw `{"status":422,...}` error. Proxmox has none of
that class of bug because every guest view carries the same toolbar and state-aware actions. The bugs live in
**objects in a particular state**, which page-level crawls never visit. So this review walks objects, not pages.

Priority: **P0** = broken or dead-end UX (the page tells the user to do something and gives no control for it, raw API
error text, a spinner that never settles, an action offered that cannot work, a destructive action with no
confirmation, a forced trip back to a list). **P1** = expected Proxmox feature missing. **P2** = polish.

Status column: **fixed** = on branch `hx-proxmox-parity-p0` (lands with its CI), **in flight** = another branch is
landing it (named), **queued** = planned, not started.

## The gap list (prioritised)

| # | Gap | Pri | Status |
|---|---|---|---|
| G1 | Every error read `HTTP 409: ...` / `HTTP 424 [reason]: ...`; unparsed bodies were shown raw | P0 | fixed: `api.rs` `ApiError { message, status, raw }`, `friendly_text()`; `message` is always a sentence |
| G2 | Stopped-VM console: no Start, endless "Connecting...", raw 422 JSON | P0 | in flight: `fix/console-stopped-state` |
| G3 | VM Summary page had no power actions at all (only "Hibernate now", also offered for a stopped VM) | P0 | fixed: `components/vm_power.rs` `VmPowerBar` on Summary |
| G4 | VM Hardware page for a stopped VM: only disabled Shutdown/Reboot/Reset, no Start; no hard Stop for a running one | P0 | fixed: same `VmPowerBar` |
| G5 | Pages for a VM that does not exist rendered its settings forms plus `HTTP 404: ...`; Hardware showed device panels stuck on "Loading..." | P0 | fixed: `#vmd-missing` / `#vm-hw-missing` empty states with a way back |
| G6 | Paused VM: no Resume anywhere (backend `POST /api/vms/:id/{pause,resume}` existed, no UI used them) | P0 | fixed: Pause/Resume in `VmPowerBar`, Resume in the VM list |
| G7 | Delete offered on a running VM (then a 409); confirm text never said the disks are destroyed | P0 | fixed: `vm_list.rs` Delete only when stopped, confirm names the disks |
| G8 | VM list failures shown in a blue info banner, wiped by the next refresh | P0 | fixed: `#vm-action-error` alert |
| G9 | Container console/terminal for a stopped container connected anyway and printed `[error] HTTP 422: ...`; runtime defaulted to Podman whatever the container was; no Start | P0 | fixed: `container_console.rs` looks the container up, Start + auto-connect, real runtime, missing-container state |
| G10 | LXC page: Start/Stop/Freeze/Thaw enabled in every state; a missing container showed all actions | P0 | fixed: `pages/lxc/mod.rs` actions follow the state |
| G11 | Snapshot "Include memory" clickable for a stopped VM, silently ignored | P0 | fixed: `snapshot_list.rs` enabled only for a running VM |
| G12 | Storage pool Remove ran on the first click | P0 | fixed: `storage/pool_management.rs` confirmation |
| G13 | Network page: "No running container. Start one on the Containers page first." as plain text | P0 | fixed: link to the Containers page |
| G14 | Alerts page: a failed load read "No active alerts" | P0 | fixed: `alerts.rs` error state |
| G15 | Per-VM tabs only on Summary; Hardware had just "Back to VMs" | P0 | fixed: `components/vm_tabs.rs` on Summary and Hardware (Console page after G2 lands) |
| G16 | Create VM: a failed disk import leaves the VM created and the form armed, so Create again makes a duplicate | P0 | queued |
| G17 | Storage "Add" offers NFS/CIFS/iSCSI/Ceph/GlusterFS/S3 with one Path field and no server/export/credentials; no Edit | P0 | queued |
| G18 | Restore of a RAM snapshot never sends `restore_memory` (always disk-only) | P1 | queued |
| G19 | No Migrate on a VM (only `/ha/migration`, hidden without the cluster feature) | P1 | queued |
| G20 | No task model in the UI: long actions (shutdown, clone, backup, migrate) are a spinner or a toast; `/tasks` exists but nothing links to a task, no live log, no Stop | P1 | queued |
| G21 | No persistent resource tree (Datacenter > node > guest) with a per-guest context menu; `/resources` is a read-only page | P1 | queued (shell work: coordinate with the ui-kit track) |
| G22 | No bulk actions (start/shutdown/stop/migrate several guests) | P1 | queued |
| G23 | Backup Now, Clone, Convert to template, Snapshots, Replication not reachable from the VM's own page | P1 | queued (VmTabs gives the place) |
| G24 | Node network: changes apply live, no pending list / Apply / automatic revert; no bond or VLAN create | P1 | queued |
| G25 | SDN: UI never calls `POST /api/sdn/apply`, so per-target results are invisible; deletes without confirmation | P1 | queued |
| G26 | Notification matchers: backend exists, no UI | P1 | in flight: `hx-notif-matchers-v4` |
| G27 | Per-VM autostart and start/shutdown order (exists for LXC, not VMs); VM notes | P1 | queued |
| G28 | Replication target node/pool are free text | P1 | queued |
| G29 | API token privilege separation and expiry | P1 | queued (verify first) |
| G30 | Console paste is a `window.prompt`; no scale mode; SPICE only as a `.vv` download | P2 | queued |
| G31 | Create VM always lands on the list, not on the new VM; no "Start after create" | P2 | queued (journey/e2e tests assume the list) |
| G32 | Boot order is three presets, not a per-device list | P2 | queued |
| G33 | Clone jobs page loads once, no Cancel/Remove (routes exist) | P2 | queued |
| G34 | ~25 pages swallow load errors (`if let Ok(..)` with no else); no request timeout in `api.rs` | P2 | queued: shared `AsyncState` (Loading / Error + Retry / Data) |

## Feature by feature

Paths are under `horcrux-api/horcrux-ui/src/` unless they start with `horcrux-api/src/` or `crates/`.

### Guests: VM lifecycle

| Proxmox VE | Horcrux today | Gap | Pri | Plan |
|---|---|---|---|---|
| Every guest view has one toolbar: Start, Shutdown (with Stop), Reboot, Pause/Resume, Console, Migrate, More (Clone, Convert to template, Remove), state-aware | List rows: Console, Start, Stop, menu (Shutdown, Reboot, Save state, Details, Hardware, Clone, Delete) (`pages/vm_list.rs`). Summary: none (G3). Hardware: Shutdown/Reboot/Reset only (G4). Console: none (G2) | No shared toolbar | P0 | `components/vm_power.rs` `VmPowerBar` (Start/Shutdown/Reboot/Pause/Resume/Stop/Reset by state, inline result, confirm on Stop/Reset) on Summary and Hardware; Console adopts it once G2 lands; Migrate/Clone/Template added to it with G19/G23 |
| Create wizard (General, OS, System, Disks, CPU, Memory, Network, Confirm), "Start after created" | One form (`pages/vm_create.rs`), sliders, ISO, NIC, display; then the list | G16, G31 | P0/P2 | G16: keep the created id, offer "Open VM" instead of a second Create; G31 with the tests |
| Remove: confirm with VMID, purge from jobs, destroy unreferenced disks | `confirm()` then `DELETE /api/vms/:id` (always deletes the disks, refuses running: `crates/horcrux-vm/src/vm/mod.rs`) | G7 + no purge options | P0/P1 | fixed G7; purge options later |
| Suspend / resume | Backend `pause`/`resume` (`horcrux-api/src/api/vm_hw.rs`), guest-state save/resume on the list | G6 | P0 | fixed |
| Summary: status, uptime, CPU/RAM/net graphs, guest-agent IPs, notes | State, CPUs, memory, disks, pool, tags, host-shutdown policy (`pages/vm_detail.rs`); agent IPs only on Hardware (`pages/vm_devices.rs`) | Graphs/IPs/notes | P1 | IPs + sparkline on Summary; `description` on VmConfig |
| Options: start at boot, order, delay | LXC only (`horcrux-common` LXC options) | G27 | P1 | same keys on VMs, honoured by the guests service |
| Hardware: add/remove per device, hot-plug | Broad (`pages/vm_hardware.rs`, `vm_devices.rs`, `vm_advanced.rs`, `vm_passthrough.rs`); CPU/RAM fields disabled while running although a hot-plug block exists below | Layout | P2 | point the disabled fields at the hot-plug block |
| Missing guest (stale link) | Forms + `HTTP 404` (G5) | G5 | P0 | fixed |

### Console

| Proxmox VE | Horcrux today | Gap | Pri | Plan |
|---|---|---|---|---|
| noVNC / xterm.js / SPICE; stopped guest shows "not running" with Start | noVNC iframe, serial, SPICE `.vv` (`pages/console.rs`, `crates/horcrux-vm/src/console/novnc.rs`) | G2 | P0 | in flight on `fix/console-stopped-state` (Start button, never connects while stopped, friendly HTML in the iframe on a race) |
| Clipboard, Ctrl-Alt-Del, scaling, fullscreen, reconnect | Paste via prompt, Ctrl+Alt+Del, fullscreen, new window | G30 | P2 | inline paste box, scale mode |
| Container console (pct console / enter) | `pages/container_console.rs` | G9 | P0 | fixed |

### Data protection

| Proxmox VE | Horcrux today | Gap | Pri | Plan |
|---|---|---|---|---|
| Per-guest Snapshots tab: tree, NOW marker, take (with RAM for running), rollback, edit, delete | Global list with VM filter (`pages/snapshot_list.rs`); tree on `/backup/snapshots` | G11, G18, no per-guest tree | P0/P1 | G11 fixed; G18; snapshot tree under the VM tab |
| Clone: full/linked, target node and storage, from snapshot | Dialog on the list: name, full/linked, start after (`vm_list.rs`); `/clone-cross-node` route unused | storage/node, progress | P1 | add selects, poll `/api/clone-jobs/:id` |
| Convert to template; templates in the tree | `/backup/templates` only | G23 | P1 | "Convert to template" in the toolbar menu, Template badge |
| Backup now from the guest; scheduled jobs; restore into new/existing id; prune; verify | `/backup` form, jobs, retention, verify (`pages/backup/*`) | G23; restore id free text; error outside the dialog | P1 | Backup tab on the VM; restore dialog keeps its own error |
| Replication tab: target node picker, schedule, rate, log | `pages/replication_list.rs`, free-text node/pool | G28 | P1 | selects from cluster nodes and pools |
| Migrate: target node, online/offline, precondition check | `/ha/migration` (`pages/ha/migration_center.rs`), `api::migrate_vm` unused by VM pages | G19 | P1 | Migrate in the toolbar with `get_migration_targets` |

### Storage

| Proxmox VE | Horcrux today | Gap | Pri | Plan |
|---|---|---|---|---|
| Datacenter > Storage: add per type with its own fields, edit (content types, nodes, enable), remove | `pages/storage/pool_management.rs`: id, type, one path field; backend request takes `name/storage_type/path/lvm` only; no PUT | G12, G17 | P0 | G12 fixed; G17: per-type field sets, hide types the API cannot create until it can, `PUT /api/storage/pools/:id` |
| Storage content: upload, download from URL, remove, restore | `pages/storage_content.rs`; download-from-URL only on the ISO page (`pages/iso_library.rs`) | URL download, restore action, upload progress, load error looks empty | P1 | reuse `components/iso_add.rs`; hide the table on error |
| SAN (multipath iSCSI/FC/NVMe-oF), Ceph | `pages/storage/san.rs`, `pages/ceph.rs` | see `proxmox-9.3-parity.md` row 4 | - | - |

### Network and firewall

| Proxmox VE | Horcrux today | Gap | Pri | Plan |
|---|---|---|---|---|
| Node > Network: create bridge/bond/VLAN, pending changes, Apply, automatic revert | Bridges (`pages/host_bridges.rs`), per-interface edit applied immediately (`pages/system/configuration.rs`, `horcrux-api/src/api/system_admin/host.rs`) | G24 | P1 | staged config + Apply with a timed revert; bond/VLAN forms |
| SDN zones/VNets/subnets, Apply | `pages/sdn_zones.rs`; backend `POST /api/sdn/apply` not called by the UI | G25 | P1 | Apply button with per-target results; confirms |
| Firewall: datacenter/node/guest rules, security groups, IP sets, aliases, log | `pages/firewall_objects.rs`, per-guest rules on Hardware, `pages/container_firewall.rs` | security groups, in-use guard on delete | P1 | after G25 |
| Attach a guest to a bridge | `pages/network.rs` | G13 | P0 | fixed |

### Containers

| Proxmox VE | Horcrux today | Gap | Pri | Plan |
|---|---|---|---|---|
| CT toolbar: Start, Shutdown, Stop, Reboot, Console, Migrate, Clone, Backup, template | List (`pages/container_list.rs`), LXC page (`pages/lxc/mod.rs`) with settings, snapshots, clone, resize | G10; no Migrate/Backup | P0/P1 | G10 fixed; Migrate/Backup with G19/G23 |
| Create CT wizard | `pages/container_create.rs`: probes runtimes, offers only working ones | static IP/VLAN/DNS fields | P2 | add fields |
| (no Proxmox equivalent) app containers: images, volumes, networks, stacks, events | `pages/container_*.rs`, 424 empty states | - | - | - |

### Cluster, HA, nodes

| Proxmox VE | Horcrux today | Gap | Pri | Plan |
|---|---|---|---|---|
| Create / join cluster with join information | `pages/ha/failover.rs` (`POST /api/cluster/create`, `/join`), `pages/ha/cluster_management.rs` | - | - | - |
| HA resources, groups/rules, fencing, status | `pages/ha/*` | HA state not shown in guest lists | P2 | state badge in the lists |
| Node maintenance from the node menu | `pages/ha/maintenance.rs` (this node) | per-node control | P1 | in flight: `hx-node-maintenance-cluster-v8` |

### Access, tasks, notifications, system

| Proxmox VE | Horcrux today | Gap | Pri | Plan |
|---|---|---|---|---|
| Users, groups, roles, path ACLs with propagate, realms, API tokens with privilege separation, TOTP/WebAuthn/recovery | `pages/auth/*` | G29 (unverified) | P1 | verify, then add privsep/expiry |
| Task log panel: every long action is a task with live log, status, Stop | `/tasks` list (`horcrux-api/src/api/tasks.rs`) | G20 | P1 | long routes return a task id; a footer task drawer opens its log |
| Notification targets + matchers + test | Channels (`pages/monitoring/notifications.rs`); matchers backend `horcrux-api/src/api/notifications.rs` | G26 | P1 | in flight: `hx-notif-matchers-v4` |
| Datacenter search above the tree | `/resources` page | no global search box | P2 | navbar search to `/resources?q=` |
| Syslog, services, updates, certificates, time, DNS, shell | `pages/system/*` (services actions follow state) | log viewer: `proxmox-9.3-parity.md` row 5 | - | - |

## What changed in QA so this class is caught by CI, not by the owner

- `ci/ux-qa/user_journey.py` step **J9 object states**: opens Summary, Hardware and Console for a stopped VM, and the
  pages of a VM and a container that do not exist. Each must offer the control its own text asks for (Start), offer
  no impossible action, settle within 6 s (no "Connecting..."/"Loading..." left), never show raw API error text, and
  have the VM tab bar or a way back. The three stopped-console checks are in the baseline until G2 lands.
- `ci/ux-qa/ux_crawl.py` rule **raw-error** on every page, desktop and phone, light and dark: `HTTP 4xx/5xx`, a JSON
  error object, or an error-code constant visible to the user fails the job.
- `horcrux-api/horcrux-ui/src/api.rs` unit test `user_facing_error_text_never_shows_http_codes_or_json` and
  `components/vm_power.rs` `every_state_offers_a_way_out_and_nothing_impossible`.
- `docs/qa/owner-findings.md` rows 36 and 37.

## One converging track

The earlier comparison ran as eleven rounds of the same visual-polish job. It churned because (1) the goal was
"look like Proxmox", which has no finish line, so every round found more styling; (2) each round produced one large
branch that sat behind a long CI queue while main moved, so it was rebased and re-run (ui-system-1 to -7), and runner
infrastructure failures (checkout probe "Unable to read current working directory") were re-run one job at a time;
(3) functional flows were never in scope, so the rounds could not have found this class.

From here there is one track with a finish line: this gap list. The ui-kit v0.3 branch lands first (it owns the shell,
grid and `vm_list.rs` layout); this branch is built on top of it, and the console fix lands on its own. Every later
gap is a small branch off main that changes one flow, adds its J9/journey check, and closes its row here. Visual
polish is no longer a separate job: a page is restyled only when its row is worked.
