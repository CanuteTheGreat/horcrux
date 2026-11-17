# Boot environments: "boot the previous version"

Owner ask (2026-11-13): NixOS-style atomic rollback. After a bad upgrade, boot the system as it was before it.
Horcrux installs on ZFS and upgrades through Portage (`emerge @world` from the update API task, the CLI or unattended
updates), so the primitive is a ZFS boot environment (BE): a root dataset that can be cloned from a snapshot and booted.

## What already existed (found while writing this)
`horcrux-be` (`build/live-iso/common/overlay/usr/lib/live-installer/liveinst/be.py`, shipped by `sys-apps/horcrux-tools`)
and the installer's ZFS layout (`zfsroot.py`) already had: BE datasets `rpool/ROOT/<name>`, a per-BE kernel + initramfs in
`/boot/be/<name>/` (hard-linked content store, so identical kernels cost nothing), a generated static `grub.cfg` with one
menu entry per BE, `list|snapshot|create|activate|rollback|destroy`, the Portage hook, `KEEP=5` pruning, /boot space
management, and a QEMU rollback test. Phase 1 closes the gaps listed below instead of building a second system.

## Design

### Layout
- `rpool/ROOT/<be>`: root file systems (`mountpoint=/`, `canmount=noauto`). Only these are cloned/rolled back.
- Shared, never rolled back: `rpool/home`, `rpool/var/log`, `rpool/var/lib/libvirt`, `rpool/var/lib/docker` (when Docker is
  installed), and (new) `rpool/var/lib/horcrux/vms` = Horcrux's default `paths.vm_storage`. Before this change the
  installer left `/var/lib/horcrux/vms` inside the root dataset: a rollback would have hidden every Horcrux VM disk.
- `/boot` (ext4, outside ZFS): `be/<name>/{vmlinuz,initramfs.img,kver}`, `be/.snap/<be>@<snap>/` (the kernel saved with each
  snapshot, so a rolled-back root gets the kernel whose modules it contains), `grub/grub.cfg`.

### Snapshot before every upgrade
`/etc/portage/bashrc` (written by the installer) runs `horcrux-be auto-snapshot` in the
`setup` phase of the first package of any emerge run: one `pre-upgrade-YYYYMMDD-HHMMSS` snapshot (at most one per 30 min,
never fatal, throttle stamp in `/run`). Because the hook lives in Portage itself it covers the API task
(`/api/system/packages/upgrade-all|:name/upgrade`), the CLI, cron/unattended updates and binpkg installs alike, with no
per-caller code to keep in sync. Retention: the newest `KEEP` (default 5) automatic snapshots; snapshots a BE was cloned
from and **pinned** ones (ZFS user property `horcrux:pin=yes`) are never pruned; manual snapshots are never pruned.

### Bootloader: GRUB (static grub.cfg), ZFSBootMenu later
ZFSBootMenu is the natural fit for BE menus (reads BEs and kernels straight from the pool, including encrypted ones) and
stays the phase 3 candidate. Phase 1 keeps GRUB because the whole installer (BIOS and EFI, T2 Macs, aarch64, LUKS swap/
hibernate resume, multi-disk `/boot` mirrors) is built on it and GRUB cannot read an encrypted pool, which is why the
kernel per BE lives on the plain `/boot` and `horcrux-be` regenerates a static `grub.cfg`. The menu has one entry per BE
(`Horcrux - <be> [kernel X]`), the active one first and default; the previous BE always stays in the menu, so a bad
rollback is itself one menu choice away.

### Rollback
`horcrux-be rollback [BE@snapshot] [--name N]`: with no argument = the newest pre-upgrade snapshot of the running BE.
It clones the snapshot to a new BE, restores the saved kernel, sets `bootfs`, rewrites grub.cfg. It never reboots and never
destroys: the BE you leave stays bootable. `activate` only switches `bootfs`/default. `destroy` refuses the running or active BE
and pinned ones.

### Data safety guard
`horcrux-be check` verifies that every path in `DATA_PATHS` (default `/home /var/lib/libvirt /var/lib/horcrux/vms
/var/lib/docker /var/lib/lxc`) is on its own dataset (or is empty/absent). `rollback` and `activate` refuse when a data path
holds files on a BE dataset (existing installs from before the vms dataset), unless `--force`. The API/UI show the same list.
Migration of an existing install's `/var/lib/horcrux/vms` into its own dataset is phase 2.

### CLI, API, UI
- CLI: `horcrux-be list [--json] | snapshot [name] | rollback [BE@snap] | activate | create --from | pin | unpin | check |
  destroy | destroy-snapshot | prune | dedupe | sync | grubcfg`.
- API (`horcrux-api/src/api/system_admin/boot_environments.rs`, RBAC: GET = SysAudit, others SysModify, via `/api/system/*`):
  `GET /api/system/boot-environments`, `POST .../snapshot`, `POST .../rollback`, `POST .../pin`, `POST .../:name/activate`,
  `DELETE .../:name`, `DELETE .../snapshots/:be/:snap`. A node without horcrux-be or ZFS root answers a structured 424
  (`boot-environments-tool-missing`, `no-zfs-boot-environments`). Names are validated like the CLI; arguments never go through a shell.
- UI: System > Boot Environments (`/system/boot-environments`): environments, snapshots, roll back / boot next / pin / destroy,
  the data-safety list. Nothing reboots; the page says so.

## OpenRC and systemd parity (owner rule)
Coordinated with `docs/design/init-parity.md` (init abstraction in `horcrux-api/src/init.rs`; not duplicated here).
Boot environments are init-neutral: Portage's bashrc, ZFS, GRUB and `horcrux-be` (Python) never touch the init system, the API module
shells out to `horcrux-be` only, and the QEMU test asserts behavior, not services. The init-specific pieces, each with both flavours:
- the installer's `zfsroot.configure()` enables the ZFS boot services through `liveinst/initsys.py`: `zfs-import`/`zfs-mount`/`zfs-zed`
  with `rc-update` on OpenRC, `zfs-import-cache.service`/`zfs-mount.service`/`zfs-zed.service` + `zfs.target`/`zfs-import.target` with
  `systemctl enable` on systemd (`UNIT_MAP`; unit-tested in `tests/test_initsys.py`, asserted on the booted guest by the QEMU gate
  `be-zfs-and-health-services-enabled-on-this-init`);
- the health service of the automatic fallback: `/etc/init.d/horcrux-be-health` (OpenRC) and `horcrux-be-health.service` (systemd) run the
  same command, `horcrux-be mark-good --wait --detach`.

## Phase 2 (this change)

### Where `/var/lib/horcrux` (the config database) lives: it rolls back WITH the root
Decision: `/var/lib/horcrux` stays on the boot environment dataset; only `/var/lib/horcrux/vms` (the disks) is shared.
- The database schema and the `horcrux-api` binary are a pair: a binary rolled back to an older version must not open a database a newer
  version already migrated. Sharing the database across environments would make every rollback of a schema-changing upgrade a corruption risk.
- Nothing is lost: the environment you leave keeps its own, newer database untouched and stays bootable (`Boot next` returns to it).
- What a rollback does revert is configuration changed after the snapshot (users, ACLs, VM definitions created since). The VM disks are
  still there (shared dataset); re-register such a VM from its disk, or boot the newer environment again. The UI says so on the roll back
  buttons, and the pre-upgrade snapshot is taken at upgrade time, so the window is "what you changed since that upgrade".

### `horcrux-be migrate-data [--dry-run] [--keep-original] [PATH...]`
Existing installs from before the installer gave `/var/lib/horcrux/vms` its own dataset keep VM disks inside the boot environment, and the
data guard (`check`) refuses a rollback for them. `migrate-data` fixes that for every `DATA_PATHS` entry that is a plain directory on a
`rpool/ROOT/<be>` dataset and holds files (`liveinst/bemigrate.py`):
1. blockers first: any process with a file open under the path (a running VM, docker), a dataset of that name that is not ours, too little
   free space for a second copy: exit 3, nothing changed (the dry run prints the same list);
2. snapshot `migrate-data-<stamp>` of the environment (never pruned: the original data stays reachable until you destroy it);
3. create `rpool/<path>` (missing parents as `canmount=off` containers, `recordsize=64K` for VM disks), copy with `rsync -aHAXS` (else `cp -a`),
   verify type/size/mode/owner/mtime/symlink target of every entry, copy again after re-checking the path is still quiet;
4. swap: old directory renamed aside, dataset mounted at the path, old copy removed (`--keep-original` keeps it); a failed mount puts it back.
Idempotent (a migrated path is skipped; an interrupted run resumes its half-copied dataset, marked `horcrux:migrating=yes`). API: `POST
/api/system/boot-environments/migrate-data {dry_run}` (the real run is a task); the UI offers it in the data-safety banner.
Old boot environments keep their own stale copy under the mount until destroyed.

### Explicit snapshot step in the upgrade task + the Packages banner
`POST /api/system/packages/upgrade-all` and `.../:name/upgrade` first run `horcrux-be pre-upgrade --json` (always snapshots as
`pre-upgrade-<stamp>`, unlike the Portage hook's 30-minute throttle, then arms that throttle so the emerge does not snapshot twice; skipped
with a reason where there is no ZFS root or `AUTO_SNAPSHOT=no`). The task answer carries `snapshot`. The Package Manager page shows a banner
for the newest `pre-upgrade-*` snapshot of the running environment with the time and a one-click "Roll back to before this upgrade" (nothing
reboots; it disappears once the rolled-back environment is the running one). The hook in Portage's bashrc stays as the safety net for CLI,
cron and unattended upgrades.

### Automatic fallback ("boot the previous version by itself")
A just-activated environment (`activate`, `rollback`) is ON TRIAL. State lives in GRUB's environment block `/boot/grub/grubenv`
(plain ext4 `/boot`, so GRUB can write it; `befallback.py`): `hx_trial` (environment under trial), `hx_tries` (one `x` per boot of it: GRUB
has no arithmetic), `hx_good` (the environment that was running = the way back), `hx_failed`. The generated `grub.cfg` loads them, appends
an `x` on every boot of the trial environment and, when `FALLBACK_TRIES` (default 3) boots passed without a healthy one, makes `hx_good`
the default entry and sets `hx_failed`. Every entry also gets `panic=10` (`PANIC_REBOOT`), so a kernel panic resets the machine and is
counted instead of hanging at a dead console.
The health service (`horcrux-be-health`, OpenRC script / systemd unit) runs `horcrux-be mark-good --wait --detach` on every boot: it waits up
to `HEALTH_TIMEOUT` (300 s) for the pool to be online and, when `horcrux-api` is enabled, for port 8006 to answer (`HEALTH_CMD` adds a custom
check), then closes the trial (`boot-ok`) or, after a fallback, makes `bootfs` follow what actually booted, flags the failed environment
(`horcrux:failed=yes`, shown as "did not boot healthy") and records a `fallback` event. An unhealthy boot that does come up records
`health-failed` and stays on trial, so the counter keeps running. Not armed when `FALLBACK=no`, when there is no previous environment with
a kernel, or when the health service is not enabled (nothing could end the trial). A hand-picked older menu entry never ends or fails a trial.
Events (`armed`, `boot-ok`, `fallback`, `health-failed`, `snapshot`, `migrated`) go to `/var/log/horcrux-be-events.jsonl` (the shared log
dataset, so they survive rollbacks and fallbacks) and show in `horcrux-be events`, `GET /api/system/boot-environments` (`events`, `fallback`)
and the Boot Environments page. Limits: a boot that hangs without panicking is not counted until someone resets the machine; multi-disk
`/boot` mirrors copy `grub.cfg` but the counter lives on the first drive's `/boot`.
Existing installs: `sys-apps/horcrux-tools` pkg_postinst enables the health service on whichever init runs and regenerates `grub.cfg`.

## Test (phase 1; phase 2 gates are listed under Phases)
`build/live-iso/common/zfs-enc-test.py` (QEMU, KVM on rw-man): install on encrypted ZFS, then in the installed system install a
probe package 1.0 through emerge, **upgrade it to 2.0 through emerge** (the Portage hook must snapshot by itself), write files
into the VM datasets and /home, `horcrux-be rollback` (no argument), reboot with the passphrase, and assert: booted the rolled-back BE,
probe package is 1.0, every VM/user file is byte-identical, the old BE is still listed. Unit tests (no VM):
`tests/test_be_boot_space.py`, `tests/test_be_rollback.py` (pins, previous-snapshot, data guard, installer layout).

## Phases
1. (done) pins, `--json`, no-arg rollback, data guard + `horcrux-be check`, vms dataset in the installer, API, UI, design, QEMU gate.
2. (done) `migrate-data`, explicit pre-upgrade snapshot + Packages banner, automatic fallback with events, `/var/lib/horcrux` decision,
   systemd ZFS services asserted; QEMU gates `be-*`, `migrate-data-*`, `explicit-pre-upgrade-snapshot-step`, `broken-environment-panicked-n-times-then-fell-back`
   run on both inits (`zfs-enc-test.py --server`, default on, `--no-be-phase2` skips).
3. ZFSBootMenu as an optional loader (EFI), per-BE `etc` diff view in the UI, an unhealthy-but-running boot optionally rebooting itself.
