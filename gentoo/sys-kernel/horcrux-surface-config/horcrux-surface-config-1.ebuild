# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

LS_COMMIT="8c62d84193a38bd097238b192a4f7262db8a9f7e"
LS_SERIES="6.18"

DESCRIPTION="Microsoft Surface support for sys-kernel/gentoo-kernel: linux-surface patch series, kernel config, initramfs config"
HOMEPAGE="https://github.com/linux-surface/linux-surface"
SRC_URI="https://github.com/linux-surface/linux-surface/archive/${LS_COMMIT}.tar.gz -> linux-surface-${LS_COMMIT}.tar.gz"
S="${WORKDIR}/linux-surface-${LS_COMMIT}"

# the patches are GPL-2 kernel patches
LICENSE="GPL-2"
SLOT="0"
KEYWORDS="~amd64"

# Used with sys-kernel/gentoo-kernel ${LS_SERIES}.x ONLY (the series is pinned to the 6.18 stable branch; the patches are
# applied by Portage's user-patch mechanism so every 6.18.y rebuild carries them). No dependency on the kernel on purpose:
# a config update must not force a 1 h source build; the kernel is built by `emerge -1 sys-kernel/gentoo-kernel`.
# The Surface Pro 1/2 needs none of the Surface Aggregator/IPTS drivers (see docs/SURFACE.md); the series is carried so
# the same kernel also serves Surface Pro 3+ hardware, harmlessly (the drivers only bind to their own ACPI/PCI ids).
RDEPEND="sys-kernel/dracut"

src_install() {
	local p n=0
	insinto /etc/portage/patches/sys-kernel/gentoo-kernel
	for p in patches/${LS_SERIES}/*.patch; do
		# keep the upstream order with a numeric prefix; user patches apply in lexical order
		(( ++n ))
		newins "${p}" "$(printf '%03d' ${n})-surface-${p##*/}"
	done
	(( n > 0 )) || die "no patches found for series ${LS_SERIES}"

	insinto /etc/kernel/config.d
	cat "configs/surface-${LS_SERIES}.config" "${FILESDIR}/surface-sp1.config" > "${T}/90-horcrux-surface.config" || die
	newins "${T}/90-horcrux-surface.config" 90-horcrux-surface.config
	insinto /usr/share/horcrux-surface
	doins "${FILESDIR}/required-config.txt"

	# initramfs: the keyboard (Type Cover = USB-HID), disk encryption and the console graphics must be in the initramfs
	insinto /etc/dracut.conf.d
	newins - 90-horcrux-surface.conf <<-'EOF2'
		# Horcrux Surface profile: Type Cover/USB keyboard at the (single) passphrase prompt, LUKS2 + LVM + resume
		hostonly="no"
		add_drivers+=" hid hid_generic usbhid hid_microsoft hid_multitouch xhci_pci xhci_hcd dm_crypt dm_mod i915 "
		add_dracutmodules+=" crypt dm lvm resume "
		omit_dracutmodules+=" plymouth "
	EOF2
}

pkg_postinst() {
	elog "Rebuild the kernel so it carries the patches: emerge -1 sys-kernel/gentoo-kernel"
	elog "Kernel command line of a Surface Pro 1/2: button.lid_init_state=open (written by the installer)"
}
