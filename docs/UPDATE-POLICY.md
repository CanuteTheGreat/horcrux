# Versions, updates and rollback

This page is the policy for how Horcrux is versioned, how a node gets updates and how to go back
when an update goes wrong. The release history is in [CHANGELOG.md](../CHANGELOG.md).

## Version numbers

Horcrux uses [Semantic Versioning](https://semver.org/): `MAJOR.MINOR.PATCH`, tagged in git as
`vMAJOR.MINOR.PATCH` (pre-releases: `v1.2.0-rc.1`). The workspace version in `Cargo.toml`, the tag
and the CHANGELOG entry always match; CI refuses a release tag where they don't.

| Change | Before 1.0 (now) | From 1.0 on |
|---|---|---|
| API or CLI change that breaks existing callers, config format change that needs manual edits, removed feature or USE flag | minor (`0.x` -> `0.x+1`) | major |
| New feature, new API endpoint, new USE flag, additive database migration | minor or patch | minor |
| Bug fix, security fix, packaging fix | patch | patch |

Before 1.0 a minor release may break compatibility; the CHANGELOG says so under the release. The
1.0 exit criteria are in [ROADMAP.md](ROADMAP.md).

## Release channels

| Channel | What it is | How often |
|---|---|---|
| Tagged release `vX.Y.Z` | Release binaries and web UI on the Forgejo release page, release notes from the CHANGELOG, live ISOs built from the tag | When a green `main` is released with `ci/release.sh` |
| Daily | `app-emulation/horcrux-bin-0.1.0_pYYYYMMDDHHMM` in the Horcrux overlay and the daily live ISOs | After each passing live-ISO build of `main` |
| Live ebuild | `app-emulation/horcrux-9999`, built from `main` | Every commit |

A tagged release is only cut from a `main` commit whose full CI run (`ci.yml`) is green.

## How a node is updated

Every installed node keeps itself current through Portage with `sys-apps/horcrux-autoupdate`
(on by default; details in [design/autoupdate.md](design/autoupdate.md)):

1. once a day: `emaint sync -a`;
2. a ZFS boot-environment snapshot `pre-upgrade-*` (no snapshot, no update);
3. `emerge -uDN @world`, binary packages first;
4. a guarded `--depclean`, `@preserved-rebuild`, trivial config file updates.

The node is never rebooted by an update. A new kernel, ZFS module, microcode or init system marks
`/var/lib/horcrux-autoupdate/reboot-required`; reboot when it suits you. Running VMs and containers
keep running while packages update. After an update of `app-emulation/horcrux` or `horcrux-bin`,
restart the API service (`rc-service horcrux-api restart` on OpenRC, `systemctl restart horcrux-api`
on systemd) to run the new version; guests are not affected by the restart.

Status of the last run: `horcrux-autoupdate check` (exit 0 ok, 1 stale, 2 failed) and
`/var/lib/horcrux-autoupdate/status.json`. Turn unattended updates off with `ENABLED=no` in
`/etc/horcrux/autoupdate.conf` and update by hand with `emerge -uDN @world` or from the web UI
(System > Packages); the boot-environment snapshot is taken the same way (a Portage hook), whichever
way the update is started.

### Database

Horcrux keeps its configuration in SQLite (`/var/lib/horcrux/horcrux.db`). Migrations run when the
API starts, are named, run once, and only add tables or columns, so data written by the previous
release keeps working (tested on every nightly run by `depth_upgrade`, which upgrades a database
made by the previous release tag). A change that removes or rewrites existing data is only allowed in
a release that is marked breaking in the CHANGELOG, and it says how to roll back.

## Rolling back

### Whole system (ZFS installs): boot environments

Every update starts with a `pre-upgrade-*` snapshot of the root file system, so the system as it
was before the update can be booted again ([design/boot-environments.md](design/boot-environments.md)):

- web UI: System > Boot Environments, "Roll back" on the snapshot, then reboot;
- CLI: `horcrux-be rollback` (newest pre-upgrade snapshot) or `horcrux-be rollback BE@snapshot`,
  then reboot.

Rollback clones the snapshot into a new boot environment and makes it the default; the environment
you leave stays in the boot menu, so undoing a rollback is one menu choice. VM disks, `/home`, logs,
libvirt and container storage live on their own datasets and are not rolled back. The Horcrux
database lives on the root file system, so it returns to its state at the snapshot: configuration
changes made after the update are undone with it.

### One package

To go back to the previous Horcrux build without rolling back the system:

1. `emerge --oneshot =app-emulation/horcrux-bin-<previous version>` (or `=app-emulation/horcrux-<x.y.z>`);
   the overlay keeps the last few daily `horcrux-bin` builds;
2. if the newer release changed the database, restore the copy from before the update
   (`/var/lib/horcrux/horcrux.db` from the `pre-upgrade-*` snapshot:
   `/.zfs/snapshot/<snapshot>/var/lib/horcrux/horcrux.db`);
3. restart `horcrux-api`.

Mask the bad version (`/etc/portage/package.mask`) so the next unattended update does not
reinstall it, and remove the mask once a fixed version is out.

### Non-ZFS installs

Without ZFS there are no boot environments. Back up `/etc/horcrux` and `/var/lib/horcrux` (minus
VM disks) before updating, and roll back by package as above.

## Supported versions and the security-fix window

Horcrux is a one-person project: there is no service-level agreement, and fixes are made on a best
effort basis. What is stated here is what the project commits to do.

| Version | Bug fixes | Security fixes |
|---|---|---|
| Latest minor release (`0.x.y`, today the newest tag) | yes, as patch releases | yes, as patch releases |
| Previous minor release | no | for 90 days after the first release of the next minor, when the fix applies without redesign |
| Anything older | no | no: update to a supported version |
| Daily channel and `horcrux-9999` | the current build only | the current build only |

From 1.0 on the same shape holds per major version: the latest minor of the current major gets all
fixes, the previous minor gets security fixes for 90 days after the next minor, and the previous
major gets security fixes for 12 months after the next major's first release.

Before 1.0 the honest limits are:

- there are no long-term support branches; compatibility between minors is not promised (see the
  version table above), so the previous-minor window may need a node to take a manual step, which
  the CHANGELOG describes;
- nodes with unattended updates enabled (the default) move to the newest build on their own within
  a day, so in practice they are always on the latest minor;
- "latest" means the newest tag in [CHANGELOG.md](../CHANGELOG.md), not an old daily build.

How a security fix reaches you: a patch release is cut as soon as the fix is ready and tested; it
reaches nodes through the daily channel and unattended updates within a day, and appears in the
CHANGELOG under the release. Report vulnerabilities as described in
[SECURITY.md](../SECURITY.md).

### Rolling back inside the supported window

A rollback by [boot environment or by package](#rolling-back) lands on a version that was supported
when you took it. If the version you roll back to has meanwhile left the table above, it receives
no further security fixes: roll back only to look for the cause, and update again as soon as a fixed
version is out.

### Related

- What a node sends over the network, and what it never sends: [PRIVACY.md](PRIVACY.md).
- Licenses of the software Horcrux is built from: `THIRD-PARTY-LICENSES.txt` (attached to every
  release, served by the web UI under System > About > Open-source licenses).

## Cutting a release (maintainers)

```
ci/release.sh 0.2.0        # on an up-to-date main whose ci.yml run is green
git push origin main v0.2.0
```

`ci/release.sh` sets the workspace version, regenerates `CHANGELOG.md` from the commits since the
last tag (`ci/changelog.py`), commits `release: v0.2.0` and creates the annotated tag. The tag push
runs `release.yml`, which checks that the tag, `Cargo.toml` and the CHANGELOG agree, builds the
release binaries and web UI, and publishes them with the CHANGELOG section as release notes.
