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

### Kernel safety: the previous kernel always stays bootable
Every boot environment keeps `KERNELS_KEEP` kernels (default 2: the newest plus the previous one; `/etc/horcrux/be.conf`) in
`/boot/be/<be>/` (newest) and `/boot/be/<be>/kernels/<kver>/` (older ones, hard links into the content store, so a kept kernel costs only its own
files). `horcrux-be sync` (kernel-install hook) rotates the current kernel into `kernels/` before saving the new one, seeds the previous kernel from the
`vmlinuz-*` pair installkernel left in `/boot` on a machine that never kept history (only when its `/lib/modules/<kver>` exists), and never prunes the
known-good kernel. `grub.cfg` gets one entry per kept kernel: `Horcrux - <be> ... [kernel X]` (id `be-<be>`) and `Horcrux - <be>, previous kernel ...
[kernel Y]` (id `be-<be>-k-<kver>`), then every other boot environment the same way.

Trial boot with automatic fallback (`KERNEL_TRIAL=yes`, default): a kernel that has not yet booted successfully is tried ONCE. `sync` sets GRUB's
`next_entry` (grubenv) to the new kernel while the default stays the last known-good kernel; GRUB clears `next_entry` before booting, so a kernel that
panics or hangs is not tried twice and the next boot is the known-good one without any key press. The boot-time service `horcrux-be-good` (OpenRC script
and systemd unit, both shipped by `sys-apps/horcrux-be`; the installer enables it) runs `horcrux-be mark-good --wait`: after `GOOD_DELAY` seconds, with the
pool ONLINE, the running kernel is recorded in `/boot/be/<be>/good` and becomes the default (grub.cfg regenerated). Without that service enabled the
newest kernel is simply the default (no trial), so a missing service can never pin the machine to an old kernel. An existing install that has no `good`
record yet treats the kernel it is running at the first `sync` as known-good. `horcrux-be boot-previous` boots the previous kernel on the next boot only;
`horcrux-be kernels [--json]` lists what is kept. Nothing here reboots the machine. Non-ZFS installs use grub-mkconfig (`horcrux-boot-entry`), which lists
every kernel in `/boot`.

ZFSBootMenu (phase 3, `BOOT_LOADER=zfsbootmenu`) is not shipped yet: it reads kernels from each boot environment itself and lists every kernel it finds,
so only the retention above applies; trial boots use GRUB's `next_entry` and are skipped for any other loader.

### The menu always boots unattended

A machine must never wait at the boot menu for a key. GRUB switches its countdown off for good as soon as an automatic boot
attempt comes back to the menu (`grub_menu_execute_entry` unsets `timeout`): a kernel or initrd that fails to load, or an EFI
stub that returns, leaves a menu with no timer. The generated grub.cfg therefore never relies on the first attempt:

- `fallback` lists the active boot environment's other kernels (known-good first) and then a retry entry, as entry
  **numbers** (GRUB ignores menu ids in `fallback` and silently ends the chain);
- the retry entry reloads the config (the countdown is armed again) and after three rounds restarts the machine;
- the environment block is read only when it exists, from the /boot file system (`load_env -f /grub/grubenv next_entry`);
- a one-shot trial kernel (`next_entry`) is booted only when GRUB could clear it first (`if save_env ...`), so a loader that
  cannot write the block never retries a hanging trial forever;
- the countdown is always positive, whatever `TIMEOUT` says.

`build/live-iso/common/grub-autoboot-test.py` boots the generated menu with each image's own GRUB and kernel under UEFI,
never pressing a key: normal, no environment block, one-shot `next_entry`, read-only disk, broken default, broken trial
kernel, every kernel broken (retry then restart), and a control without `fallback` that must hang.

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
Nothing in boot environments touches the init system: Portage's bashrc, ZFS, GRUB and `horcrux-be` (Python) are init-neutral,
the API module shells out to `horcrux-be` only, and the QEMU test asserts behavior, not services. The one init-specific piece
is the installer's `zfsroot.configure()` enabling `zfs-import`/`zfs-mount`/`zfs-zed` with `rc-update`: the systemd installer
path (init-parity phase 3) must enable `zfs-import-cache.service`/`zfs-mount.service`/`zfs-zed.service`/`zfs.target` instead;
until then BE install is OpenRC-only on the installer side, but a systemd host that has `horcrux-tools` and a ZFS root runs
every other part unchanged.

## Test
`build/live-iso/common/zfs-enc-test.py` (QEMU, KVM on rw-man): install on encrypted ZFS, then in the installed system install a
probe package 1.0 through emerge, **upgrade it to 2.0 through emerge** (the Portage hook must snapshot by itself), write files
into the VM datasets and /home, `horcrux-be rollback` (no argument), reboot with the passphrase, and assert: booted the rolled-back BE,
probe package is 1.0, every VM/user file is byte-identical, the old BE is still listed. Unit tests (no VM):
`tests/test_be_boot_space.py`, `tests/test_be_rollback.py` (pins, previous-snapshot, data guard, installer layout).

## Phases
1. (this change) pins, `--json`, no-arg rollback, data guard + `horcrux-be check`, vms dataset in the installer, API, UI, design, QEMU gate.
2. Existing installs: `horcrux-be migrate-data` moving `/var/lib/horcrux/vms` into its own dataset (offline-safe, VMs stopped);
   API-side explicit "snapshot then upgrade" task step and a banner in the Packages page showing the snapshot taken;
   boot-time health check that auto-selects the previous BE after N failed boots (GRUB `grubenv` fallback counter);
   `/var/lib/horcrux` (config DB) split decision: today it rolls back with root so DB and binary stay consistent.
3. ZFSBootMenu as an optional loader (EFI), systemd installer path, per-BE `etc` diff view in the UI.
