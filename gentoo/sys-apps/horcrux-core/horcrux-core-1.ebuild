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
#   core            this package: server + hypervisor, NO display stack, NO desktop, NO machine-specific app (steam, rustinion, brave, ... live in a machine layer)
#   desktop edition gnome-extra/horcrux-gnome-defaults (GNOME) depends on THIS package; the XFCE edition for small devices does the same
#   machine layers  the owner's own machines (gringotts, hogshead, surface, hogsmeade) add their apps on top of an edition
# Every fix that applies to every Horcrux install belongs here (or in a package listed here), never in an edition: the 2026-11-03 gringotts fixes (clock
# hook, first-boot network wait, kept dev files, zfs dist-kernel USE) had been attached to the GNOME package and a server install would have missed them.
RDEPEND="
	net-misc/chrony
	net-misc/horcrux-time
	net-misc/horcrux-chrony-nm
	sys-apps/horcrux-firstboot
	sys-apps/horcrux-repo-config
	sys-apps/horcrux-backup
	sys-fs/zfs
	sys-kernel/dracut
	sys-kernel/installkernel
	# an installed system must be able to build an out-of-tree kernel module (zfs-kmod after every dist-kernel update, DKMS-style drivers) against its running kernel:
	# gentoo-kernel-bin modules carry BTF, which `make modules` finishes with pahole. It used to be on an image only when the zfs-kmod binary package of the day happened
	# to be built from source (QEMU gate kernel-module-builds-against-running-kernel failed on a server rebuilt from the package cache, 2026-11-04).
	dev-util/pahole
"

src_install() {
	insinto /etc/portage/package.use
	newins "${FILESDIR}"/package.use horcrux-core
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
