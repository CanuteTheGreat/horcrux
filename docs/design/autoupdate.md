# Automatic updates (sys-apps/horcrux-autoupdate)

Every Horcrux install keeps itself current through Portage. Updates are on by default; the machine is never rebooted.
Pulled in by `sys-apps/horcrux-core` (-r5), so desktop and server installs both have it.

## What runs
Trigger: hourly, one per init system (init parity): `/etc/cron.d/horcrux-autoupdate` (OpenRC, cronie) or
`horcrux-autoupdate.timer` (systemd, enabled at install). The program starts an update only when `INTERVAL_HOURS` (24) have passed
since the last attempt, so a machine that was off at night catches up when it is next on.

One run (`horcrux-autoupdate run`), at nice 19 / ionice idle, under `flock` and skipped while another emerge runs (or on battery):
1. `emaint sync -a`. A repository that returns non-zero (typically an rsync mirror caught mid-update, whose tree fails Manifest
   verification so Portage keeps the old one) is synced again on its own, `SYNC_RETRIES` (2) times after `SYNC_RETRY_DELAY` (120 s);
   only then does the run fail, with the repository and the reason (`gentoo: Manifest mismatch for www-apps/Manifest.gz`) in the error.
   A non-zero exit with every repository at code 0 is a post-sync hook (eix-update): a warning (`check` exits 1), not a failed sync,
   as long as the gentoo tree's `metadata/timestamp.chk` is at most `SYNC_MAX_AGE_HOURS` (48) old.
   The package also installs `/etc/portage/repo.postsync.d/50-horcrux-md5-cache`: git overlays that ship no metadata cache get one
   from `egencache` after each sync (kept out of `git status`; never the gentoo repo or a repo whose cache is tracked in git). Without it
   eix-update falls back to `ebuild ... depend`, which current Portage refuses, and prints `cannot properly execute` per ebuild.
2. ZFS root: boot-environment snapshot `pre-upgrade-*` through `horcrux-be snapshot` (plain `zfs snapshot` if the tool is absent). No snapshot, no update.
3. `emerge --update --changed-use --newuse --deep --with-bdeps=y --keep-going @world` (the owner's `-uUND --with-bdeps=y`, the default on every install; `CHANGED_USE=no` drops `--changed-use`; an explicit `WITH_BDEPS=no` selects `--with-bdeps=n` for a host that must not carry build-only packages. On a headless server the first run therefore also merges ~80 build-only packages, compilers, headers and X11 libraries; the headless gate checks the shipped image, not a later update, and the depclean steps pass the same `--with-bdeps` switch so they never propose those packages for removal), binpkgs from the binhost first (`--getbinpkg --usepkg`); `app-emulation/horcrux-bin` is excluded from binpkgs (its ebuild fetches a tarball).
4. `emerge --depclean <packages>`: the `--pretend` list minus the protected kernel/ZFS/boot/firmware/Horcrux packages (`DEPCLEAN_KEEP`,
   kept and named in the status), when that is at most `DEPCLEAN_MAX` (20) packages. A bigger set usually means a package is missing
   from the world file: nothing is removed.
5. `@preserved-rebuild` when Portage preserved libraries.
6. `etc-update --preen` (trivial changes only; `ETC_UPDATE=all` selects `--automode -5`). Pending config files are counted in the status.

## Reboots
Never executed. A new kernel, microcode, ZFS module, NVIDIA driver or systemd update is written to
`/var/lib/horcrux-autoupdate/reboot-required` (one reason per line) and `/run/reboot-required`. A later boot clears the marker.
No notification pops up; desktop shells and agents read the file.

## Failure visibility
`/var/lib/horcrux-autoupdate/status.json` (result, steps, merged, pending, reboot reasons, last_success, log path) and, after a failed
run, `/var/lib/horcrux-autoupdate/failed` (removed by the next good run). Logs: `/var/log/horcrux-autoupdate/run-*.log` (14 kept) and syslog tag `horcrux-autoupdate`.
`horcrux-autoupdate check` prints one line and exits 0 ok / 1 warning (no success for `STALE_DAYS`, 3) / 2 critical (failed, interrupted, or 2x stale):
wire it into a health check or a Rustinion custom check. Nothing is mailed.

## Config
`/etc/horcrux/autoupdate.conf` (all optional; see the file). `ENABLED=no` turns it off. Machine-specific values belong in a private overlay, not here.

## Tests
`gentoo/sys-apps/horcrux-autoupdate/tests` (unit, no Portage) and `tools/vmtest/scenarios/autoupdate_openrc.py` (OpenRC guest, real KVM: run, lock, failed marker, reboot marker, crond trigger, with stand-in portage tools). The systemd units are checked by `gentoo/ci/check-init-parity.py` and `systemd-analyze`.
