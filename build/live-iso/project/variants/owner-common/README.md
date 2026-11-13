# owner-common: the owner's personal apps, shared by his own machines (NOT a public image)

Horcrux is a headless hypervisor/server OS with an optional desktop (`docs/ISO-INSTALL.md`, "Layers"). Public images (server, gnome-light, ...) carry NO
personal application. The owner's own picks are added by the machine layers (`gringotts`, `hogshead`, `hogsmeade`, the Surface/madammalkins profile) from here:

- `packages.txt`   apps every one of his general-purpose machines has (inventories: ctg-infrastructure `hosts/<host>/inventory-*.md`). A machine adds its
                   own extras in its own `packages.txt` / Dockerfile (hogshead: keepalived, sshpass, rustup; hogsmeade: Steam ONLY there, never here).
- `portage/`       keywords + USE those apps need (rustup is ~amd64; Nextcloud without the Dolphin plugin)
- `overlay/`       files the apps' backup needs: `/etc/ctg-backup/required` (what a restore must hold so the owner never signs in again)

A machine Dockerfile does `COPY project/variants/owner-common/portage/ /etc/portage/`, `COPY project/variants/owner-common/overlay/ /` and emerges
`packages.txt`. `project/public-forbidden.txt` lists what must never appear in a public image; the core/desktop Dockerfiles and the QEMU gates enforce it.

## Full GNOME (owner rule 2026-11-12)
Every non-server owner machine (hogsmeade, hogshead, gringotts) runs FULL GNOME: `gnome-base/gnome` + `gnome-extra/horcrux-gnome-extras` (variant `gnome`,
`variants/gnome/packages.txt`, installed by the gringotts/hogshead app steps and hogsmeade's desktop step (NOT listed in this directory's `packages.txt`: a public edition is not an owner app and the test-machine denylist covers that file); USE `gnome-base/gnome -extras` so Epiphany and the systemd-only apps stay out). The public
`gnome-light` edition is unchanged. The Surface/madammalkins image runs XFCE and never takes GNOME or this layer (tests: common/tests/test_full_gnome.py).

## Test machines never use this layer (owner order 2026-11-05)
gringotts, hogshead and madammalkins are TEST machines while they are used for testing: they get the PUBLIC editions (gnome-light or the full gnome edition, the XFCE
small-device profile) + `app-misc/horcrux-test-machine` + their machine HARDWARE enablement (hogshead: T2 kernel/Touch Bar/fans + the private
`app-misc/hogshead-machine` firmware meta package; Surface: `horcrux-surface-config`), and NO app from this directory. `app-misc/horcrux-test-machine`
ships the guard: `horcrux-test-machine-check` (denylist of personal packages/paths + an ALLOWLIST for the world file, policy in
`gentoo/app-misc/horcrux-test-machine/files/`), weak Portage blockers on the big personal apps, and the lab's hx-invariants job runs the check
on every machine. Building a gringotts/hogshead image from this layer for a TEST machine is a mistake; the owner's real devices (HOGSMEADE,
beauxbatons, and these laptops once they stop being test machines) keep using it.
