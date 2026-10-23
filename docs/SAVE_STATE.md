# Save state on host shutdown, resume on boot (and cluster drain)

Horcrux does what Hyper-V's "Automatic stop action = Save state" does, for VMs
**and** containers: when the host shuts down or reboots, every running guest's
memory and device state is written to disk, and when the host boots again the
guests are resumed exactly where they were - same processes, same RAM, same
uptime. In a cluster the host first tries to **move** its VMs **and
containers** to other nodes (live, no restart where the runtime allows it) and
only saves what cannot move.

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
| LXC | `lxc-checkpoint -s` (`criu dump` directly for a container that was itself restored) | `lxc-checkpoint -r` | the chosen storage |
| Incus | `incus stop --stateful` | `incus start` | in the instance |
| LXD | `lxc stop --stateful` | `lxc start` | in the instance |
| Docker | `docker checkpoint create --checkpoint-dir` | checkpoint copied into the daemon's store, `docker start --checkpoint` | the chosen storage |
| Podman | `podman container checkpoint` | `podman container restore` | Podman's storage |

All of them need **CRIU** (`emerge sys-process/criu`, `apt install criu`) and a
kernel with checkpoint/restore support (`criu check` must pass; a container
inside another container cannot do it, a VM or bare metal can); Docker
additionally needs `"experimental": true` in `/etc/docker/daemon.json`. Horcrux
probes this per node (`criu check`, cached 30 s) and per guest, and shows -
next to each guest and on the Guest Power page - whether state can be saved
and, if not, exactly why. Without CRIU (or when a container does not fit) the
host-shutdown pass does a **clean shutdown** of the container and the boot
pass **starts** it again (it was running), with the reason in the UI; a manual
*Save state* is refused with the same reason and the container keeps running.

What each runtime needs from the container, found by running real checkpoints
(all of this is exercised by `guest_state_ct_vm_e2e.sh`):

* **LXC** must be **privileged** (`unprivileged: false`: CRIU cannot dump
  user-namespaced containers) with the **`checkpoint` feature** (Configure ->
  *checkpointable*; applies at the next start): it renders
  `lxc.console.path = none` and `lxc.tty.max = 0`. CRIU cannot dump ttys whose
  masters live in `lxc-start`, and `lxc-checkpoint` refuses to restore a
  container that had a console - so a checkpointable container has no Console
  tab / console log (the Terminal tab, `lxc-attach`, still works). Horcrux also
  works around two LXC 6.0 problems: it names the generated AppArmor profile
  explicitly for the restore (LXC hands CRIU the literal `generated`; the
  profile is kept next to the checkpoint because LXC deletes it when the
  container stops and the kernel forgets it at reboot), and it runs `criu dump`
  itself for a container that was already restored once (`lxc-checkpoint` fails
  with "Failed getting freezer path" on those).
* **Incus / LXD** instances must be **privileged with no console**:
  `incus config set NAME security.privileged=true raw.lxc="lxc.console.path = none"`
  (and `migration.stateful=true`, set by Horcrux) before they are started;
  otherwise the restore fails ("Can't open file root on restore", "lxc.console.path
  configured on source host but not target"). The UI shows this as the blocker
  with the exact command. **Limit:** an instance that was itself restored
  cannot be checkpointed again (liblxc "Failed getting freezer path" on cgroup
  v2): the next host shutdown shuts it down cleanly (reason recorded) until it
  has been restarted once.
* **Docker**: `docker start --checkpoint-dir` is refused by current daemons
  ("custom checkpointdir is not supported"), so the checkpoint is copied into
  the daemon's own store for the restore.
* **Podman** installs its default AppArmor profile when a container is
  *created*, never on restore, and the kernel forgets it at reboot: Horcrux
  creates and removes a throw-away container first so a resume after a reboot
  works. Podman containers are tracked again after an API restart (they used
  to vanish from the list and from the boot pass).

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
  new guests - not from drain, not from rebalance, and **HA recovery never
  restarts a dead node's guests on it** (the maintenance flag is asked of every
  online peer on each HA tick). *Drain* live-migrates its running VMs to other
  nodes through the QEMU-native migration engine (memory/device state over TCP,
  local disks mirrored, shared storage not copied) and moves its running
  **containers** (below), placed by free RAM (10 % headroom),
  architecture and the **placement rules**: *pin* to a node, *affinity group*
  (stay together), *anti-affinity group* (never share a node), *never migrate*.
  A dry-run plan is shown first (`GET /api/nodes/maintenance/plan`).
* **Drain on shutdown** (node default on, clustered nodes only): the stop pass
  drains first; whatever cannot move (no room, a rule, a failed or timed-out
  move, LXD/Incus VMs, a target without the container's runtime) is handled by
  the local stop action, so a shutdown never loses a guest. The node leaves its shutdown maintenance when
  it boots; explicit maintenance stays until you leave it.
* **One owner at all times**: the migration resumes the target only after the
  source froze and finished sending; a failed or cancelled move leaves the
  source running and the target without a trace; an HA VM's placement number is
  raised by the target in the same step, and the HA supervisors stop any copy on
  a node the cluster has not placed the VM on (fencing/HA decide if the source
  dies mid-move). Every move is recorded in the location history
  (`GET /api/cluster/moves`).
* **Container moves** (`api/ct_move.rs`, `container/transfer.rs`; peer routes
  under `/api/ha/peer/ct/`): the target is asked first (`prepare`: runtime
  installed, CRIU usable, no container of that name). Then, per runtime:
  Incus/LXD - `incus move` (live with CRIU on both nodes; if the live move fails
  or CRIU is missing, stateful or plain stop -> `incus move` -> start on the
  target; the target trusts the source through a one-time token and
  `core.https_address` is set on demand); LXC - `lxc-checkpoint`, then the
  container directory + checkpoint are streamed to the target, which restores it
  (`lxc-checkpoint -r`); Docker - `docker checkpoint`, the committed container
  filesystem (`docker commit` + `save`), named/anonymous **volume data** and the
  checkpoint are streamed, the target creates the container from the same
  options and restores it (the checkpoint's cgroup scope and mount sources are
  re-pointed at the new container id, otherwise it could never be checkpointed
  again); Podman - `podman container checkpoint --export` plus the image and
  volumes. Without CRIU on either node, or when the checkpoint fails (the
  container still runs), it is **stop -> copy -> start** (filesystem and volumes
  survive, the processes restart). Host **bind mounts** are host state and are
  not copied (logged). The **single-owner rule** is the VM one: the container
  is stopped (checkpointed) on the source before anything is copied, the target
  starts it only after the whole bundle arrived, the source's stopped copy is
  deleted only after the target answered "running"; if the target fails (no
  room, daemon down, restore error) the source first has the target remove
  whatever it holds (`abort`) and only then resumes its own checkpoint - never
  two running copies, never none - and if the target cannot be asked the
  container stays stopped on the source rather than risk two.
* **Rebalance**: evens out RAM load with live migrations (VMs and containers), at most *moves at
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

## Limits

* Incus/LXD: a restored instance cannot be checkpointed (or live-migrated)
  again until restarted - saves/moves fall back to a clean shutdown / a cold
  move. Needs privileged instances without a console (see above).
* LXC: checkpointable containers have no console tab. Unprivileged LXC
  containers and many network setups cannot be dumped - the UI says so.
* Host bind mounts of a moved container are not copied; a container whose
  target lacks its runtime stays and is saved/shut down locally.
* Podman keeps its checkpoint in its own storage; LXD/Incus keep theirs inside
  the instance (size not shown).

## Tests

* `horcrux-api/tests/guest_state_e2e.sh` (CI, guest-power workflow):
  real QEMU + CirrOS - save/resume keeps a RAM marker, a process PID and the
  uptime; free-space refusal; the unit's stop path then the boot path with
  several guests and policies; crash fallback; stale-state refusal. Also run
  under TCG (`HX_NO_KVM=1`, nightly/on demand).
* `horcrux-api/tests/guest_state_ct_vm_e2e.sh` (CI, guest-power workflow): a
  throwaway Debian VM (own kernel, `hx_vm.sh`) with LXC, Incus, Docker, Podman
  and CRIU; for each runtime a process holding a counter and a random marker in
  memory survives Save state / Resume and the host shutdown / boot paths. Never
  skipped: `criu check` must pass.
* `horcrux-api/tests/drain_ct_vm_e2e.sh` (CI, guest-power workflow): two
  clustered VMs; per runtime a drain moves the container live (marker, counter,
  a filesystem file and a volume survive), the other node's drain moves it back
  (a node in maintenance is no target), a target without room makes the move
  fail and the source resumes the original (one running copy at every point),
  and a target without CRIU gets a cold stop -> copy -> start.
* `horcrux-api/tests/drain_e2e.sh` (CI, guest-power workflow): two nodes - drain
  moves VMs with RAM markers without a restart, the shutdown pass, boot,
  rebalance back, and a target killed mid-move leaving exactly one running copy.
* `horcrux-api/tests/ha_failover_e2e.py` (CI, ha-failover workflow): HA recovery
  after a node failure and after a partition never lands on a node in
  maintenance.
* `horcrux-api/tests/guest_state_ct_e2e.sh` (CI, ci.yml Test job): the LXC
  fallback without CRIU inside a container (the reason shown, clean shutdown +
  restart).
* Unit tests: policy, boot decisions, systemd/OpenRC unit ordering, the
  placement planner and `recovery_targets` (`cluster/drain.rs`), QEMU save
  helpers, CRIU probing, the container bundle/manifest and image id rewrite
  (`container/transfer.rs`).
