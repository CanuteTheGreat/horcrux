# Horcrux roles and the manager package

Every Horcrux edition is a headless server first (`sys-apps/horcrux-core`); optional capability arrives as one package per role.

| Package | What it is | Where it ships |
|---|---|---|
| `sys-apps/horcrux-core` | headless base: clock, first boot, backup, overlay sync, ZFS rules | every image |
| `app-emulation/horcrux-bin` | the **Horcrux manager** (API server, web UI, CLI), prebuilt; depends on `qemu[spice,usbredir,virtfs,vnc]`, libvirt, dnsmasq, bridge-utils | every image (installed by the core layer), so server, GNOME and every machine layer carry it |
| `sys-apps/horcrux-role-containers` | Incus, Podman, Buildah, Skopeo, LXC (Docker is in the server image) | on demand: `emerge sys-apps/horcrux-role-containers` |
| `app-emulation/horcrux` | the source build of the manager (cargo + wasm UI; USE flags pick backends); blocked by `horcrux-bin` | on demand, for developers |

## Manager
* Server profile enables `horcrux-api` + `libvirtd`; a desktop edition installs the package but leaves the services off (`rc-update add libvirtd default; rc-update add horcrux-api default`).
* Upgrade on an installed machine: `emerge -1 app-emulation/horcrux-bin` with a newer `horcrux-manager-<arch>.tar.zst` in `/usr/share/horcrux/prebuilt/` (or `HORCRUX_MANAGER_TARBALL=`); without one the daily-release asset `horcrux-manager-daily-<arch>.tar.zst` is fetched (`HORCRUX_MANAGER_URL=` overrides).
* The image build (`build/live-iso/project/Dockerfile.app`) cross-builds the tarball on Debian bookworm with the portable CPU baseline; the core layer emerges it and then drops the tarball.

## Keywords / USE for roles
`sys-apps/horcrux-repo-config` (already on every machine) ships `package.accept_keywords/horcrux-roles` and `package.use/horcrux-roles`: Portage reads them while resolving the role package, so they cannot ride along inside it. `gentoo/ci/portage-role-checks.sh` proves the role package resolves with only those files.
