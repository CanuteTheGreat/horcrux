# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Horcrux core: the headless hypervisor/server base every Horcrux install is made of (clock, first boot, backup, overlay sync, ZFS rules); desktops depend on it"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64 arm64"

# LAYERING (owner 2026-11-03: "It should work on a headless server too. It shouldn't be a desktop-only OS."):
#   core            this package: server + hypervisor, NO display stack, NO desktop, NO machine-specific app (steam, rustinion, ... live in a machine layer)
#   desktop edition gnome-extra/horcrux-gnome-defaults (GNOME) depends on THIS package; the XFCE edition for small devices does the same
#   machine layers  the owner's own machines (gringotts, hogshead, surface, hogsmeade) add their apps on top of an edition
# Every fix that applies to every Horcrux install belongs here (or in a package listed here), never in an edition: the 2026-11-03 gringotts fixes (clock
# hook, first-boot network wait, kept dev files, zfs dist-kernel USE) had been attached to the GNOME package and a server install would have missed them.
# sys-apps/horcrux-release: the package-owned /usr/lib/os-release (name, version, build id, variant); every Horcrux install has it.
# dev-util/pahole:
#   an installed system must be able to build an out-of-tree kernel module (zfs-kmod after every dist-kernel update, DKMS-style drivers) against its running kernel:
#   gentoo-kernel-bin modules carry BTF, which `make modules` finishes with pahole. It used to be on an image only when the zfs-kmod binary package of the day happened
#   to be built from source (QEMU gate kernel-module-builds-against-running-kernel failed on a server rebuilt from the package cache, 2026-11-04).
# /etc/docker/daemon.json ("ip-forward-no-drop"):
#   Docker >= 28 sets the iptables FORWARD policy to DROP when it enables IP forwarding. nftables evaluates EVERY base chain a packet hits, so that DROP
#   also kills traffic libvirt/Horcrux already accepted in its own table: every NAT-network VM (virbr0, vmbr*) lost the internet whenever dockerd ran
#   (HOGSMEADE 2026-11-12: captainjacksparrow could not reach ProtonVPN). With this option Docker leaves the policy alone; it still isolates its own
#   bridges (DOCKER-FORWARD / DOCKER-ISOLATION / per-network DROP of unpublished ports). The file is harmless where Docker is not installed
#   (QEMU gate docker-does-not-break-vm-nat-forwarding checks it everywhere and does the netns forwarding test where Docker exists).
# sys-auth/horcrux-ssh-pam (-r2): ssh logins do not start or hold the user's OpenRC user services (pam_openrc stops them only with the user's LAST
#   login, so an ssh session kept a logged-out GNOME session's gnome-shell alive; also keeps servers' ssh logins from spawning user services).
RDEPEND="
	sys-auth/horcrux-ssh-pam
	net-misc/chrony
	net-misc/horcrux-time
	net-misc/horcrux-chrony-nm
	sys-apps/horcrux-firstboot
	sys-apps/horcrux-repo-config
	sys-apps/horcrux-backup
	sys-fs/zfs
	sys-kernel/dracut
	sys-kernel/installkernel
	dev-util/pahole
	sys-apps/horcrux-release
"

src_install() {
	insinto /etc/portage/package.use
	newins "${FILESDIR}"/package.use horcrux-core
	insinto /etc/docker
	newins "${FILESDIR}"/docker-daemon.json daemon.json
	# marker: this is a Horcrux install (the installer, the QEMU gates and `horcrux-time-config` read it)
	insinto /usr/share/horcrux
	newins - core-release <<-EOF
		horcrux-core ${PVR}
	EOF
}

pkg_postinst() {
	elog "Horcrux core installed. Check the clock rules: horcrux-time-config check; enable at boot: rc-update add chronyd default; rc-update add horcrux-timesync default;"
	elog "rc-update add horcrux-firstboot default (the installer does all three)."
}
