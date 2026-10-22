# Save state on host shutdown, resume on boot (and cluster drain)

Horcrux does what Hyper-V's "Automatic stop action = Save state" does, for VMs
**and** containers: when the host shuts down or reboots, every running guest's
memory and device state is written to disk, and when the host boots again the
guests are resumed exactly where they were - same processes, same RAM, same
uptime. In a cluster the host first tries to **move** its VMs to other nodes
(live migration, no restart) and only saves what cannot move.

UI: **Infrastructure > Guest Power (Save State)** (`/system/power`) and
**Infrastructure > High Availability > Maintenance & Rebalance**
(`/ha/maintenance`). CLI: `horcrux power ...`. API: below.

## Policy

Per guest, overriding a **node default**:

| Setting | Values |
|---|---|
| Automatic stop action | **Save state** (default) / Shut down / Power off |
| Automatic start action | **Resume if it was running** (default) / Always start / Nothing |
| Start delay | seconds after the boot pass began |
| Start order | lower starts first (default 1000) |
| Save states to | a directory on the storage of your choice (default `<data dir>/saved-state/data`) |

Node-only: save timeout per guest (300 s), shutdown timeout per guest (120 s),
guests handled at once (4), *drain on shutdown* (on) and its per-move timeout.

* **Save state** falls back to *Shut down* when the state cannot be saved, and
  *Shut down* falls back to *Power off* after the shutdown timeout. The reason
  is kept per guest and shown in the UI ("State could not be saved: ...").
* **Resume if it was running**: a saved state is resumed; a guest that was
  running but has no saved state (it was shut down as the fallback, or the host
  crashed) is started normally; a guest that was stopped stays stopped.
* **Always start**: resume if there is saved state, otherwise start.
* **Nothing**: the guest stays stopped; a saved state is kept.

## How state is saved

### QEMU VMs (KVM and TCG)

The vCPUs are frozen (QMP `stop`), the whole machine state - RAM, CPU, every
device - is streamed to `<storage>/vm-<id>/vmstate.bin` with QMP `migrate`
(`exec:cat > file`), QEMU exits, and the file is renamed into place only when
complete. On resume a fresh QEMU is started with the same command line plus
`-S -incoming defer`, the file is loaded with `migrate-incoming` and the guest
continues. This works the same under TCG (no `/dev/kvm`) and KVM, on every QEMU
release.

* **Free space** (guest RAM + 64 MB) is checked on the target storage before
  anything is touched; a refusal names the numbers and the guest keeps running.
* **Safety**: the saved record keeps the VM's hardware configuration and the
  size and modification time of every disk (and the UEFI variable store). A
  state whose hardware changed or whose disk was modified since is **refused**
  at resume - loading old RAM over newer disks would corrupt them. It is kept,
  with the reason, until you *Discard* it (the VM then boots normally).
* A failed save continues the guest where it was. A failed resume kills the
  half-started QEMU, keeps the state and records the error.
* **Never resumed twice**: the index record is deleted first, then the state
  file, once the guest is running again. Starting a guest that has saved state
  (Start button, API, `horcrux vm start`) resumes it.
* LXD/Incus VMs use `incus stop --stateful` (they need `migration.stateful=true`
  from their first start; Horcrux sets it when you choose Save state and says
  so if the VM must be restarted once).

### Containers (CRIU)

| Runtime | Save | Resume | State lives |
|---|---|---|---|
| LXC | `lxc-checkpoint -s` | `lxc-checkpoint -r` | the chosen storage |
| Incus | `incus stop --stateful` | `incus start` | in the instance |
| LXD | `lxc stop --stateful` | `lxc start` | in the instance |
| Docker | `docker checkpoint create` | `docker start --checkpoint` | the chosen storage |
| Podman | `podman container checkpoint` | `podman container restore` | Podman's storage |

All of them need **CRIU** (`emerge sys-process/criu`) and a kernel with
checkpoint/restore support; Docker additionally needs
`"experimental": true` in `/etc/docker/daemon.json`. LXC containers must be
privileged. Horcrux probes this per node (`criu check`, cached 30 s) and per
runtime, and shows - next to each guest and on the Guest Power page - whether
state can be saved and, if not, exactly why. Without CRIU the host-shutdown
pass does a **clean shutdown** of the container and the boot pass **starts** it
again (it was running), with the reason in the UI; a manual *Save state* is
refused with the same reason and the container keeps running.

Discarding Incus/LXD state runs `start --stateless` (which boots the guest) and
stops it again, because that is the only way those runtimes drop their state.

## Host integration

`horcrux-api guests stop|boot|status` talks to the running API over loopback
with a root-only token (`<data dir>/saved-state/host-agent.token`). The units
order it so shutdown runs it **before** the things it needs go down:

* systemd: `deploy/systemd/horcrux-guests.service` (`systemctl enable
  horcrux-guests`): `After=horcrux.service network-online.target local-fs.target
  remote-fs.target docker.service containerd.service incus.service lxd.service
  lxc.service` - systemd stops units in reverse start order, so `ExecStop`
  (`horcrux-api guests stop`) runs while the API, the runtimes, storage and the
  network are still up. `TimeoutStopSec=1800`, `KillMode=none`.
* OpenRC (Gentoo): `rc-update add horcrux-guests default`
  (`deploy/openrc/horcrux-guests`: `need horcrux localmount`, `after docker incus
  lxd lxc netmount`; OpenRC stops in the reverse order).

If the API runs sandboxed with `ProtectSystem=strict` (the shipped
`horcrux.service`), a custom *Save states to* directory must be added to its
`ReadWritePaths`.

**Stop pass**: all running guests in parallel (bounded by *guests at once*),
each with its timeouts; a manifest of what was running is written first and
completed afterwards. **Boot pass** (the API runs it itself ~8 s after start,
`HORCRUX_GUEST_AUTOSTART=0` leaves it to the unit; `horcrux-api guests boot` is
idempotent per boot): guests are handled in start order, each at its start
delay; resume failures keep their state and are reported, never retried
silently. **Unclean crash**: no clean manifest exists, but a snapshot of the
running guests is refreshed every 20 s, so *Resume if it was running* starts
whatever was running (as a normal start - there is nothing to resume).

## Cluster: drain on shutdown, maintenance, rebalance

* **Maintenance** (UI button, `POST /api/nodes/maintenance`): the node takes no
  new guests. *Drain* live-migrates its running VMs to other nodes through the
  QEMU-native migration engine (memory/device state over TCP, local disks
  mirrored, shared storage not copied), placed by free RAM (10 % headroom),
  architecture and the **placement rules**: *pin* to a node, *affinity group*
  (stay together), *anti-affinity group* (never share a node), *never migrate*.
  A dry-run plan is shown first (`GET /api/nodes/maintenance/plan`).
* **Drain on shutdown** (node default on, clustered nodes only): the stop pass
  drains first; whatever cannot move (no room, a rule, a failed or timed-out
  move, LXD/Incus VMs, containers) is handled by the local stop action, so a
  shutdown never loses a guest. The node leaves its shutdown maintenance when
  it boots; explicit maintenance stays until you leave it.
* **One owner at all times**: the migration resumes the target only after the
  source froze and finished sending; a failed or cancelled move leaves the
  source running and the target without a trace; an HA VM's placement number is
  raised by the target in the same step, and the HA supervisors stop any copy on
  a node the cluster has not placed the VM on (fencing/HA decide if the source
  dies mid-move). Every move is recorded in the location history
  (`GET /api/cluster/moves`).
* **Rebalance**: evens out RAM load with live migrations only, at most *moves at
  once*, honouring the rules. Modes: *off* (only "Rebalance now"), *on return*
  (when a node leaves maintenance or joins), *automatic* (also whenever the
  spread exceeds the threshold). The leader (lowest-named online node not in
  maintenance) runs the automatic passes. `POST /api/cluster/rebalance/run`
  (`{"dry_run": true}` for the plan).

## API

| | |
|---|---|
| `GET/PUT /api/nodes/guest-power` | node defaults + capabilities (CRIU, runtimes) + every guest with policy, saved state, blockers, last result |
| `GET/PUT /api/{vms,containers}/:id/power-policy` | per-guest override and effective policy |
| `POST /api/{vms,containers}/:id/save-state` | save now and stop (`{"save_dir": "/mnt/x"}` optional) |
| `POST /api/{vms,containers}/:id/resume-state` | resume |
| `GET/DELETE /api/{vms,containers}/:id/saved-state` | inspect / discard |
| `POST /api/nodes/guest-power/boot-guests` | run the boot pass now |
| `GET/POST /api/nodes/maintenance`, `GET .../plan` | maintenance / drain |
| `GET/PUT /api/cluster/rebalance`, `POST .../run`, `GET /api/cluster/moves` | rebalance, history |
| `GET/PUT /api/vms/:id/placement` | pin / affinity / anti-affinity / never migrate |

CLI: `horcrux power status|save|resume|discard|policy|node-default`.

## Not (yet) covered

* Containers are not moved to other nodes by drain/rebalance (no container
  transport between Horcrux nodes yet); they are saved locally (CRIU) or shut
  down.
* HA recovery does not skip nodes that are in maintenance.
* Podman keeps its checkpoint in its own storage; LXD/Incus keep theirs inside
  the instance (size not shown).
* CRIU needs a kernel with checkpoint/restore support; unprivileged LXC
  containers and many network setups cannot be dumped - the UI says so.

## Tests

* `horcrux-api/tests/guest_state_e2e.sh` (CI, guest-power workflow):
  real QEMU + CirrOS - save/resume keeps a RAM marker, a process PID and the
  uptime; free-space refusal; the unit's stop path then the boot path with
  several guests and policies; crash fallback; stale-state refusal.
* `horcrux-api/tests/guest_state_ct_e2e.sh` (CI): LXC without CRIU (fallback,
  reason shown) and, when `criu check` passes on the runner, with CRIU (same
  PID, counter continues); otherwise that half is skipped loudly.
* `horcrux-api/tests/drain_e2e.sh` (CI, guest-power workflow): two nodes - drain moves VMs with RAM
  markers without a restart, the shutdown pass, boot, rebalance back, and a
  target killed mid-move leaving exactly one running copy.
* Unit tests: policy, boot decisions, systemd/OpenRC unit ordering, the
  placement planner (`cluster/drain.rs`), QEMU save helpers, CRIU probing.
