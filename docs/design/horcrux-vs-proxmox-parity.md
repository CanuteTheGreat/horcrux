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

Status column: the owning work lane and its tracker row (lane A = VM lifecycle and console, B = storage, backup,
snapshots, clone, C = networking, containers, migration, D = cluster, access, tasks, notifications, design system).
**in flight** = a branch is landing it. A tested reference implementation for most P0 rows exists on branch
`hx-proxmox-parity-p0` (built during this review, before the lanes started); the lanes take from it what fits their
branches instead of merging it, so one change lands per area.

## The gap list (prioritised)

| # | Gap | Pri | Status |
|---|---|---|---|
| G1 | Every error read `HTTP 409: ...` / `HTTP 424 [reason]: ...`; unparsed bodies were shown raw | P0 | lane A (A7) |
| G2 | Stopped-VM console: no Start, endless "Connecting...", raw 422 JSON | P0 | in flight: `fix/console-stopped-state` (A1-A3) |
| G3 | VM Summary page had no power actions at all (only "Hibernate now", also offered for a stopped VM) | P0 | lane A (A4, A8) |
| G4 | VM Hardware page for a stopped VM: only disabled Shutdown/Reboot/Reset, no Start; no hard Stop for a running one | P0 | lane A (A8) |
| G5 | Pages for a VM that does not exist rendered its settings forms plus `HTTP 404: ...`; Hardware showed device panels stuck on "Loading..." | P0 | lane A (A7, A12) |
| G6 | Paused VM: no Resume anywhere (backend `POST /api/vms/:id/{pause,resume}` existed, no UI used them) | P0 | lane A (A8) |
| G7 | Delete offered on a running VM (then a 409); confirm text never said the disks are destroyed | P0 | lane A (A10) |
| G8 | VM list failures shown in a blue info banner, wiped by the next refresh | P0 | lane A (A10) |
| G9 | Container console/terminal for a stopped container connected anyway and printed `[error] HTTP 422: ...`; runtime defaulted to Podman whatever the container was; no Start | P0 | lane C (C11) |
| G10 | LXC page: Start/Stop/Freeze/Thaw enabled in every state; a missing container showed all actions | P0 | lane C (C12) |
| G11 | Snapshot "Include memory" clickable for a stopped VM, silently ignored | P0 | lane B (B16) |
| G12 | Storage pool Remove ran on the first click | P0 | lane B (B17) |
| G13 | Network page: "No running container. Start one on the Containers page first." as plain text | P0 | lane A (A7) |
| G14 | Alerts page: a failed load read "No active alerts" | P0 | lane D (D9) |
| G15 | Per-VM tabs only on Summary; Hardware had just "Back to VMs" | P0 | lane A (A9) |
| G16 | Create VM: a failed disk import leaves the VM created and the form armed, so Create again makes a duplicate | P0 | lane A (A11) |
| G17 | Storage "Add" offers NFS/CIFS/iSCSI/Ceph/GlusterFS/S3 with one Path field and no server/export/credentials; no Edit | P0 | lane B (B18) |
| G18 | Restore of a RAM snapshot never sends `restore_memory` (always disk-only) | P1 | lane B (B16) |
| G19 | No Migrate on a VM (only `/ha/migration`, hidden without the cluster feature) | P1 | in flight: `lanec-parity-1` (C3) |
| G20 | No task model in the UI: long actions (shutdown, clone, backup, migrate) are a spinner or a toast; `/tasks` exists but nothing links to a task, no live log, no Stop | P1 | in flight: `hx-tasks-guest-ops` (D4, D4a) |
| G21 | Resource tree: the ui-kit v0.3 shell adds a left Resources tree (Datacenter > node > guests, storage); a per-guest context menu (Start, Console, Migrate, Clone, Remove) is still missing | P1 | in flight: tree with the ui-kit (D5a); context menu lane A |
| G22 | No bulk actions (start/shutdown/stop/migrate several guests) | P1 | lane A (A4) |
| G23 | Backup Now, Clone, Convert to template, Snapshots, Replication not reachable from the VM's own page | P1 | lane B (B14) |
| G24 | Node network: changes apply live, no pending list / Apply / automatic revert; no bond or VLAN create | P1 | lane C (C1, C8, C14) |
| G25 | SDN: UI never calls `POST /api/sdn/apply`, so per-target results are invisible; deletes without confirmation | P1 | lane C (C13) |
| G26 | Notification matchers: backend exists, no UI | P1 | in flight: `hx-notif-matchers-v4` (D11) |
| G27 | Per-VM autostart and start/shutdown order (exists for LXC, not VMs); VM notes | P1 | lane A |
| G28 | Replication target node/pool are free text | P1 | lane B |
| G29 | API token privilege separation and expiry | P1 | lane D (D3) |
| G30 | Console paste is a `window.prompt`; no scale mode; SPICE only as a `.vv` download | P2 | lane A (A6) |
| G31 | Create VM always lands on the list, not on the new VM; no "Start after create" | P2 | lane A (A5) |
| G32 | Boot order is three presets, not a per-device list | P2 | lane A (A5) |
| G33 | Clone jobs page loads once, no Cancel/Remove (routes exist) | P2 | lane B (B11) |
| G34 | ~25 pages swallow load errors (`if let Ok(..)` with no else); no request timeout in `api.rs` | P2 | lane D (D10) |

## Feature by feature

Paths are under `horcrux-api/horcrux-ui/src/` unless they start with `horcrux-api/src/` or `crates/`.

### Guests: VM lifecycle

| Proxmox VE | Horcrux today | Gap | Pri | Plan |
|---|---|---|---|---|
| Every guest view has one toolbar: Start, Shutdown (with Stop), Reboot, Pause/Resume, Console, Migrate, More (Clone, Convert to template, Remove), state-aware | List rows: Console, Start, Stop, menu (Shutdown, Reboot, Save state, Details, Hardware, Clone, Delete) (`pages/vm_list.rs`). Summary: none (G3). Hardware: Shutdown/Reboot/Reset only (G4). Console: none (G2) | No shared toolbar | P0 | lane A: one shared `VmPowerBar` (Start/Shutdown/Reboot/Pause/Resume/Stop/Reset by state, inline result, confirm on Stop/Reset) on Summary and Hardware; Console adopts it once G2 lands; Migrate/Clone/Template added to it with G19/G23 |
| Create wizard (General, OS, System, Disks, CPU, Memory, Network, Confirm), "Start after created" | One form (`pages/vm_create.rs`), sliders, ISO, NIC, display; then the list | G16, G31 | P0/P2 | G16: keep the created id, offer "Open VM" instead of a second Create; G31 with the tests |
| Remove: confirm with VMID, purge from jobs, destroy unreferenced disks | `confirm()` then `DELETE /api/vms/:id` (always deletes the disks, refuses running: `crates/horcrux-vm/src/vm/mod.rs`) | G7 + no purge options | P0/P1 | lane A (A10); purge options later |
| Suspend / resume | Backend `pause`/`resume` (`horcrux-api/src/api/vm_hw.rs`), guest-state save/resume on the list | G6 | P0 | see the gap list |
| Summary: status, uptime, CPU/RAM/net graphs, guest-agent IPs, notes | State, CPUs, memory, disks, pool, tags, host-shutdown policy (`pages/vm_detail.rs`); agent IPs only on Hardware (`pages/vm_devices.rs`) | Graphs/IPs/notes | P1 | IPs + sparkline on Summary; `description` on VmConfig |
| Options: start at boot, order, delay | LXC only (`horcrux-common` LXC options) | G27 | P1 | same keys on VMs, honoured by the guests service |
| Hardware: add/remove per device, hot-plug | Broad (`pages/vm_hardware.rs`, `vm_devices.rs`, `vm_advanced.rs`, `vm_passthrough.rs`); CPU/RAM fields disabled while running although a hot-plug block exists below | Layout | P2 | point the disabled fields at the hot-plug block |
| Missing guest (stale link) | Forms + `HTTP 404` (G5) | G5 | P0 | see the gap list |

### Console

| Proxmox VE | Horcrux today | Gap | Pri | Plan |
|---|---|---|---|---|
| noVNC / xterm.js / SPICE; stopped guest shows "not running" with Start | noVNC iframe, serial, SPICE `.vv` (`pages/console.rs`, `crates/horcrux-vm/src/console/novnc.rs`) | G2 | P0 | in flight on `fix/console-stopped-state` (Start button, never connects while stopped, friendly HTML in the iframe on a race) |
| Clipboard, Ctrl-Alt-Del, scaling, fullscreen, reconnect | Paste via prompt, Ctrl+Alt+Del, fullscreen, new window | G30 | P2 | inline paste box, scale mode |
| Container console (pct console / enter) | `pages/container_console.rs` | G9 | P0 | see the gap list |

### Data protection

| Proxmox VE | Horcrux today | Gap | Pri | Plan |
|---|---|---|---|---|
| Per-guest Snapshots tab: tree, NOW marker, take (with RAM for running), rollback, edit, delete | Global list with VM filter (`pages/snapshot_list.rs`); tree on `/backup/snapshots` | G11, G18, no per-guest tree | P0/P1 | G11/G18 lane B (B16); snapshot tree under the VM tab |
| Clone: full/linked, target node and storage, from snapshot | Dialog on the list: name, full/linked, start after (`vm_list.rs`); `/clone-cross-node` route unused | storage/node, progress | P1 | add selects, poll `/api/clone-jobs/:id` |
| Convert to template; templates in the tree | `/backup/templates` only | G23 | P1 | "Convert to template" in the toolbar menu, Template badge |
| Backup now from the guest; scheduled jobs; restore into new/existing id; prune; verify | `/backup` form, jobs, retention, verify (`pages/backup/*`) | G23; restore id free text; error outside the dialog | P1 | Backup tab on the VM; restore dialog keeps its own error |
| Replication tab: target node picker, schedule, rate, log | `pages/replication_list.rs`, free-text node/pool | G28 | P1 | selects from cluster nodes and pools |
| Migrate: target node, online/offline, precondition check | `/ha/migration` (`pages/ha/migration_center.rs`), `api::migrate_vm` unused by VM pages | G19 | P1 | Migrate in the toolbar with `get_migration_targets` |

### Storage

| Proxmox VE | Horcrux today | Gap | Pri | Plan |
|---|---|---|---|---|
| Datacenter > Storage: add per type with its own fields, edit (content types, nodes, enable), remove | `pages/storage/pool_management.rs`: id, type, one path field; backend request takes `name/storage_type/path/lvm` only; no PUT | G12, G17 | P0 | G12/G17 lane B (B17, B18): per-type field sets, hide types the API cannot create until it can, `PUT /api/storage/pools/:id` |
| Storage content: upload, download from URL, remove, restore | `pages/storage_content.rs`; download-from-URL only on the ISO page (`pages/iso_library.rs`) | URL download, restore action, upload progress, load error looks empty | P1 | reuse `components/iso_add.rs`; hide the table on error |
| SAN (multipath iSCSI/FC/NVMe-oF), Ceph | `pages/storage/san.rs`, `pages/ceph.rs` | see `proxmox-9.3-parity.md` row 4 | - | - |

### Network and firewall

| Proxmox VE | Horcrux today | Gap | Pri | Plan |
|---|---|---|---|---|
| Node > Network: create bridge/bond/VLAN, pending changes, Apply, automatic revert | Bridges (`pages/host_bridges.rs`), per-interface edit applied immediately (`pages/system/configuration.rs`, `horcrux-api/src/api/system_admin/host.rs`) | G24 | P1 | staged config + Apply with a timed revert; bond/VLAN forms |
| SDN zones/VNets/subnets, Apply | `pages/sdn_zones.rs`; backend `POST /api/sdn/apply` not called by the UI | G25 | P1 | Apply button with per-target results; confirms |
| Firewall: datacenter/node/guest rules, security groups, IP sets, aliases, log | `pages/firewall_objects.rs`, per-guest rules on Hardware, `pages/container_firewall.rs` | security groups, in-use guard on delete | P1 | after G25 |
| Attach a guest to a bridge | `pages/network.rs` | G13 | P0 | see the gap list |

### Containers

| Proxmox VE | Horcrux today | Gap | Pri | Plan |
|---|---|---|---|---|
| CT toolbar: Start, Shutdown, Stop, Reboot, Console, Migrate, Clone, Backup, template | List (`pages/container_list.rs`), LXC page (`pages/lxc/mod.rs`) with settings, snapshots, clone, resize | G10; no Migrate/Backup | P0/P1 | G10 lane C (C12); Migrate/Backup with G19/G23 |
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

- `ci/ux-qa/user_journey.py` step **J9 object states** (runs in the ui-e2e job on every push): opens Summary, Hardware
  and Console for a stopped VM, and the pages of a VM and a container that do not exist. Each must offer the control
  its own text asks for (an enabled Start), offer no action the state cannot take, settle within 6 s (no
  "Connecting..."/"Loading..." left), never show raw API error text (status codes, a JSON error object, error-code
  constants; iframe text included), and a missing object must be explained with a link back. The checks are semantic
  (button names, page text), so any implementation passes them. The ones that fail on `main` today are listed in
  `ci/ux-qa/user-journey-baseline.json` with their lane; each lane deletes its entry when its fix lands.
- `docs/qa/owner-findings.md` row 37.
- Next for QA (lane A, A7): a crawl-wide raw-error rule in `ci/ux-qa/ux_crawl.py` once the error text fix is on
  `main` (it would fail every page with an error banner today).

## One converging track

The earlier comparison ran as eleven rounds of the same visual-polish job. It churned because (1) the goal was
"look like Proxmox", which has no finish line, so every round found more styling; (2) each round produced one large
branch that sat behind a long CI queue while main moved, so it was rebased and re-run (ui-system-1 to -7), and runner
infrastructure failures (checkout probe "Unable to read current working directory") were re-run one job at a time;
(3) functional flows were never in scope, so the rounds could not have found this class.

From here there is one gap list with a finish line: this page (the map) and the shared lane tracker (live status),
worked by four lanes split by area so files rarely collide (A VM lifecycle and console, B storage and data
protection, C networking, containers and migration, D cluster, access, tasks and the design system). The ui-kit work
is folded into lane D (D5) instead of running as its own round. Each gap is a small branch off `main` that changes one
flow, keeps or adds its journey check, and closes its row. A page is restyled only when its row is worked.
