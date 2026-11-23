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
| Automatic stop action (`host_shutdown_action`) | **Save state** (default) / Shut down / Power off / Do nothing (the guest ends with the host) |
| Automatic start action | **Resume if it was running** (default) / Always start / Nothing |
| Start delay | seconds after the boot pass began |
| Start order | lower starts first (default 1000) |
| Save states to | a directory on the storage of your choice (default `<data dir>/saved-state/data`) |

Node-only (all of them settable in the UI, the API, `horcrux power node-default`
and the declarative `guest_power.node` section; nothing is built in):

| Setting | Default |
|---|---|
| `save_timeout_secs` per guest | 300 |
| `shutdown_timeout_secs` per guest (then power off) | 120 |
| `max_parallel` guests handled at once | 4 |
| `save_fallback` when a state cannot be saved: `shutdown` / `poweroff` | `shutdown` |
| `save_encryption`: `require` (encrypted storage only) / `allow_unencrypted` | `require` |
| `resume_stagger_secs` between two resumes at boot | 0 |
| `start_delay_secs`, per-guest `start_order` / delay | 0 / 1000 |
| `save_dir` | `<data dir>/saved-state/data` |
| `drain_on_shutdown` (clusters) and `drain_timeout_secs` | on / 900 |
| `stop_margin_secs` added to the computed shutdown wait | 60 |
| `guest_hibernate` passthrough VMs hibernate themselves (per-VM override too) | on |
| `hibernate_timeout_secs` per guest (then the save fallback) | 300 |
| `hibernate_probe_secs` per question to the guest agent | 15 |
| `guest_agent_path_linux` / `guest_agent_path_windows` of the Horcrux guest agent | `/usr/bin/horcrux-guest-agent` / `C:\Program Files\Horcrux\Guest Tools\horcrux-guest-agent.exe` |

* **Save state** falls back to *Shut down* when the state cannot be saved, and
  *Shut down* falls back to *Power off* after the shutdown timeout. The reason
  is kept per guest and shown in the UI ("State could not be saved: ...").
* **Resume if it was running**: a saved state is resumed; a guest that was
  running but has no saved state (it was shut down as the fallback, or the host
  crashed) is started normally; a guest that was stopped stays stopped.
* **Always start**: resume if there is saved state, otherwise start.
* **Nothing**: the guest stays stopped; a saved state is kept.

## Saved memory is encrypted at rest, or not written

A saved memory image contains whatever was in the guest's RAM: the keys of an
unlocked encrypted disk, TLS keys, passwords. Horcrux therefore writes it
**only to encrypted storage**:

* a ZFS dataset with native encryption whose key is loaded (`encryption` is
  not `off`, `keystatus=available`), or
* a LUKS / dm-crypt volume (the block device under the target has a `crypt`
  layer).

The target is checked before every save (manual and at host shutdown). A
target on plain storage, on tmpfs/overlay (not persistent) or on network
storage is **refused** with the reason; at host shutdown the guest then follows
the *save fallback* (clean shutdown by default), and the guest list shows
"will shut down, not save: ..." beforehand. Runtimes that keep the state
themselves (Incus/LXD instances, Podman) are checked against the runtime's own
storage directory. An administrator who accepts plain RAM images on a given
storage can set `save_encryption: allow_unencrypted` (node-wide); that is never
the default. (An ephemeral key held only in RAM cannot be used: it would not
survive the reboot, which is the whole point of the save.)

The state directory is root-only (0700), the image file is created 0600 before
the first byte is written, and everything is deleted after a successful resume.

## What happens to a guest at host shutdown

1. HA cluster: running VMs are live-migrated to other nodes first (*drain on
   shutdown*); what cannot move continues below.
2. Guests are handled in the reverse of their start order, `max_parallel` at a
   time.
3. **Save**: the guest is saved if it can be. It cannot when it owns a host
   device whose driver cannot save device state (a whole GPU or other PCI
   device handed over as it is; see *Host devices* below), when the target is
   not encrypted, when there is no room, when CRIU is missing (containers) or
   when the save exceeds `save_timeout_secs` (the guest keeps running
   meanwhile and is then handled by the fallback).
   A VM whose host devices cannot save their state is not saved by the host:
   its **guest hibernates itself** (next section) and only if that is not
   possible the fallback applies.
4. **Fallback** (`save_fallback`): clean shutdown (ACPI + guest agent) with its
   own timeout, then power off. The reason is logged, stored per guest, shown in
   the UI and printed by `horcrux-api guests stop`. VMs with host devices are flagged
   in the UI up front, with the kernel's answer per device.
5. At boot the guest is resumed (or started, if it had been shut down) in start
   order, after storage and network are up (the unit orders it).

## Guests that own hardware: guest-cooperative hibernate

Order of methods for a VM the host cannot save: **host save** (a partitioned or
mediated GPU that QEMU can migrate; not available for whole-device passthrough)
> **guest hibernate** through the Horcrux guest tools > **clean shutdown**
(the save fallback). The Guest Power page, the VM page and
`GET /api/nodes/guest-power` (`guests[].host_stop_action` = `hibernate`,
`host_stop_reason`, `can_hibernate`, `hibernate_detail`) say which one each VM
gets and why.

How it works. At host shutdown the node asks the guest agent
(`horcrux-guest-agent --hibernate-status`, then `--hibernate`) over the
hypervisor's guest-agent channel, which exists whether or not a console viewer is
open. The agent checks that the guest can hibernate, answers, and a moment later
(`hibernate_delay_ms`) starts the guest's own hibernate. The guest writes its
memory to its own disk and powers the VM off; the node waits up to
`hibernate_timeout_secs` for that. At the next host boot the VM is started like
any running guest and the guest **resumes by itself** with its processes and
memory (the boot pass records "Started: the guest resumes from its own
hibernation"). If the channel does not allow running commands but offers
`guest-suspend-disk`, that is used instead; the guest's swap/resume setup then
cannot be checked from outside, and the plan says so.

| Guest | Hibernate by | Prepared by the install step |
|---|---|---|
| Linux, systemd | `systemctl hibernate` | swap (a partition, or a swap file of `hibernate_swap_pct` % of RAM at `hibernate_swapfile`), `resume=` (+ `resume_offset=`) in the boot configuration (grubby or GRUB), resume support in the initramfs (initramfs-tools, dracut or mkinitcpio) |
| Linux, OpenRC/elogind | `loginctl hibernate`, `pm-hibernate` or `s2disk` (or `hibernate_command`) | the same |
| Windows | `shutdown /h` | `powercfg /h on` (`hibernate_hiberfile_pct` sets the size) |

The Horcrux install step (Guest tools > Install, which runs the installer from the
tools CD) ends with `horcrux-guest-agent --hibernate-setup`; `hibernate_at_install =
false` in the guest's `guest-tools.conf` turns that off, and `--hibernate-enable`
does the same by hand. A Linux guest needs **one restart** after the setup before it
can resume (the new boot configuration); until then the readiness check says "restart
the guest once to finish enabling it". `horcrux-guest-agent --hibernate-status` prints
the guest's own readiness report. Manual: `horcrux power hibernate <vm>` /
`POST /api/vms/:id/hibernate` hibernates a running VM now (start it to resume);
`GET /api/vms/:id/hibernate-status` asks the guest.

Honest limits: a guest **without the guest tools**, without swap/hiberfil, with a kernel
that offers no hibernation (Secure-Boot lockdown), or whose agent does not answer is
shut down cleanly instead (the reason is shown up front and after the shutdown). A guest
that does not power off within the timeout is shut down by the fallback while it may
still be writing its image, so a timeout that is too short for the guest's RAM costs a
clean shutdown, not data. A hibernation image the guest cannot use at boot is discarded by the guest, which then boots normally (the disk is always consistent). Hibernation images live on the guest's own disk, so their
encryption is the guest's (an encrypted guest disk is the way to protect them); the
host-side encrypted-storage rule applies to host-saved RAM only. Linux guests need
GRUB or grubby for the automatic boot configuration (other boot loaders get the exact
`resume=` argument to add); swap files on ZFS, tmpfs or overlay file systems cannot hold
a hibernation image. Containers cannot hibernate this way.

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
  (`deploy/openrc/horcrux-guests`: `need localmount`, `use`+`after horcrux
  horcrux-api docker incus lxd lxc netmount`; OpenRC stops in the reverse order).
* The manager enables (and starts) `horcrux-guests` by itself once per node
  wherever it is itself a boot service, and the horcrux-bin package does the
  same on install, so a VM is never just killed at host power-off (marker
  `<data dir>/saved-state/.guests-service-enabled`; an administrator who
  disables the service later is not overridden; `HORCRUX_GUESTS_NO_SERVICE=1`
  opts out).

**Waiting for the saves.** The stop client asks the API for the node's *stop
budget* (worst case from the configured timeouts: waves of `max_parallel`
guests, each `save + shutdown (+ drain)` seconds, plus `stop_margin_secs`) and
waits exactly that long, so changing a timeout changes the wait on both init
systems; the units themselves carry no fixed timeout (systemd:
`TimeoutStopSec=infinity`, OpenRC: no limit). systemd ends a shutdown job after
30 minutes by default (`JobTimeoutSec` of the shutdown targets): when the budget
exceeds that, the Guest Power page says so.

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
| `POST /api/vms/:id/hibernate`, `GET /api/vms/:id/hibernate-status` | guest hibernate now / ask the guest whether it can |
| `GET/POST /api/nodes/maintenance`, `GET .../plan` | maintenance / drain |
| `GET/PUT /api/cluster/rebalance`, `POST .../run`, `GET /api/cluster/moves` | rebalance, history |
| `GET/PUT /api/vms/:id/placement` | pin / affinity / anti-affinity / never migrate |

CLI: `horcrux power status|save|resume|hibernate|discard|policy|node-default`.

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

* Guest hibernate: unit tests (`api/guest_state/hibernate.rs`, `policy.rs`, the plan
  in `engine.rs`, the agent's planners in `crates/horcrux-guest-tools/src/power.rs`) and
  the vmtest scenario `tools/vmtest/scenarios/host_shutdown_hibernate.py`
  (OpenRC and systemd hosts): a Debian guest with a fake passthrough device
  prepares itself with `--hibernate-enable`, restarts, the host stop pass makes it
  hibernate by itself (VM stopped, plan and outcome say `hibernate`/`hibernated`),
  a real host restart follows, and the guest's memory marker and counter are
  continuous across it.

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

## Host devices (SR-IOV functions, vGPU / mediated devices)

A VM whose only host devices are VFIO devices with **VFIO device migration
v2** (`VFIO_DEVICE_FEATURE_MIGRATION`) is saved and resumed like any other VM:
QEMU streams the device state into the same file. Which devices can do that is
read from the kernel, never guessed from vendor or model:

1. The device's `vfio-dev` node in sysfs names its character device.
2. The migration feature is requested from it with the VFIO ioctls (iommufd
   device interface, or the legacy group interface on older kernels). `ENOTTY`
   means the driver has no migration support; `STOP_COPY` in the flags means it
   has. P2P and PRE_COPY are reported as extra flags.
3. A device a running QEMU holds cannot be asked again: the answer taken just
   before the VM started is used (stored next to the disk). With no such
   answer the verdict is `unverified` and QEMU decides when the save is tried
   (it refuses a save it cannot do and the guest keeps running; the fallback
   then applies).

The guest-power guest list (`device_save` in each VM's entry) and the power page
show `verdict` (`possible`, `unverified`, `impossible`, `not_applicable`), a
stable `code` (`no_migration_support`, `not_vfio_bound`, `device_missing`,
`in_use`, `no_vfio_interface`, `probe_failed`, `disabled_by_policy`) and the
reason per device. One device that cannot be saved (a whole GPU, or its audio
function) makes the VM unsaveable; it then follows the save fallback (the
guest-cooperative hibernate step is a separate feature).

The node setting `device_save` (`auto`, the default, or `never`) is part of
the guest-power node policy, so it is in the declarative config, the API and
the UI like the other power settings.

After a host reboot the mediated devices and SR-IOV virtual functions a saved
VM used no longer exist. The save records how to make them again (mediated
device parent + type + UUID; physical function + VF count + driver) and the
resume re-creates them before QEMU starts. A physical function that already
has fewer virtual functions than the VM needs is not resized (other VMs may
use them): the resume is refused with that reason and the state is kept.

Not covered here: live migration between nodes of a VM with host devices
(unchanged), and whole-GPU passthrough, which stays unsaveable.
