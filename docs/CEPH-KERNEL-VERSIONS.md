# Ceph releases, kernel line and platform versions

## Ceph: choose the release, install, create or join, upgrade

Storage > Ceph > *Ceph release* (API `GET /api/ceph/releases`, `POST /api/ceph/install`, CLI `horcrux ceph releases|install|upgrade`,
declarative `ceph.release`).

- The catalog lists every Ceph major release (Squid 19, Tentacle 20, Umbrella 21, ...) that this node's package repositories
  carry for its architecture, with the newest version, whether it is stable or testing for the architecture, and the repository.
  A release the node cannot install answers 424 (`ceph-release-unavailable`) with the releases it can.
- `POST /api/ceph/install {"release": "21", "bootstrap": {...}}` runs as a task (Tasks page, one package task at a time):
  1. Horcrux writes `/etc/portage/package.accept_keywords/horcrux-ceph-release` and `/etc/portage/package.mask/horcrux-ceph-release`, which keep
     Ceph on the chosen major release (updates stay inside it; the nightly update never jumps to the next release).
  2. `emerge sys-cluster/ceph` (binary package first).
  3. `bootstrap.mode`: `new` creates a cluster (first monitor and manager on this node, `public_network` required,
     `cluster_network`, `mon_ip`, `pool_size`/`pool_min_size`, monitor ports v2 3300 / v1 6789 settable); `join` adds this node's
     monitor and manager to the cluster a member belongs to (`peer` = that member's Horcrux API, plus an administrator login
     there; the cluster's configuration and admin keyring come from `POST /api/ceph/join-material` on the member);
     `none` installs packages only.
- Upgrades (`GET|POST /api/ceph/upgrade`) follow Ceph's order: packages on every node, then restart monitors on every node,
  then managers, OSDs, MDS, RGW, and a final step (`require-osd-release`, `noout` cleared) once every daemon runs the new
  release. The `GET` shows the daemon versions and the next step; at most two releases forward at once, never a downgrade.
- Install, upgrade and join material need `Sys.Modify` on `/nodes` (they run the package manager as root; join material is the
  admin keyring). Reading the catalog needs `Datastore.Audit`.

Declarative: `ceph: { release: "21" }` installs the packages (plan: `Create ceph`) or upgrades them (`Update ceph`); a lower
release than the installed one is reported and never planned; creating or joining a cluster stays an explicit action.

## Kernel line

System > Packages > *Kernel line* (API `GET|PUT /api/system/kernel-line`, `POST /api/system/kernel-line/apply`, CLI
`horcrux kernel status|set|apply`, declarative `system.kernel_line`, installer page *Kernel* and answer file `[system] kernel_line`).

| key | default | meaning |
| --- | --- | --- |
| `line` | `default` | `default` = the distribution's stable kernel; `newest` = opt-in, the newest kernel the repositories carry |
| `reconcile_minutes` | `60` | how often the manager re-checks the pins (0 = only on save/apply) |

- `newest` writes `/etc/portage/package.accept_keywords/horcrux-kernel-line` (testing keyword for the dist-kernel packages) and,
  when OpenZFS is installed, `/etc/portage/package.mask/horcrux-kernel-line`, which holds the kernel at the newest line the
  installed `sys-fs/zfs-kmod` builds against (its `MODULES_KERNEL_MAX`). A ZFS root therefore never gets a kernel without its
  module; when an OpenZFS update supports a newer line, the next check lifts the cap. `default` removes both files.
- The kernel arrives with the normal update (nightly `horcrux-autoupdate`, or *Update now* = the same run now): a boot-environment
  snapshot first, the previous kernel stays in the boot menu. Nothing reboots by itself; the page says when a reboot is due.
- The installer writes the same files and `/var/lib/horcrux/kernel-line.json`; the manager adopts them on first start.

## Platform versions

System > About > *This node* (API `GET /api/system/versions`, CLI `horcrux versions`): the installed versions of the kernel
(and the running one), QEMU, libvirt, Incus, LXC, LXD, Podman, Docker, OpenZFS (and the loaded module) and Ceph, read from the
node's package database.
