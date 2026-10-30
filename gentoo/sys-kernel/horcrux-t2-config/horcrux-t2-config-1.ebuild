# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

T2_COMMIT="e16720782ee30fac38486b6c58d32443899c136a"

DESCRIPTION="Apple T2 Mac support for sys-kernel/gentoo-kernel: t2linux patch set, kernel config, initramfs config"
HOMEPAGE="https://github.com/t2linux/linux-t2-patches https://wiki.t2linux.org"
SRC_URI="https://github.com/t2linux/linux-t2-patches/archive/${T2_COMMIT}.tar.gz -> linux-t2-patches-${T2_COMMIT}.tar.gz"
S="${WORKDIR}/linux-t2-patches-${T2_COMMIT}"

# the patches are GPL-2 kernel patches
LICENSE="GPL-2"
SLOT="0"
KEYWORDS="~amd64"

# Patch set pinned to the last commit before the 7.3 rebase: it applies to gentoo-kernel 7.2.x (OpenZFS 2.3.x
# declares Linux-Maximum 7.2). It is applied by Portage's user-patch mechanism, so a newer 7.2.y rebuilds with it.
# deliberately NO dependency on the kernel (it would pull a ~1 h source build into every config update). Use with
# sys-kernel/gentoo-kernel 7.2.x only: the pinned patch set does not apply to 7.3+ and OpenZFS 2.3.x does not build there
# (package.mask >=sys-kernel/gentoo-kernel-7.3 on the machine; the Horcrux image ships that mask).
RDEPEND="sys-kernel/dracut"

src_install() {
	# t2linux patches (APFS 8001/8002 skipped: macOS is gone, out-of-tree driver)
	insinto /etc/portage/patches/sys-kernel/gentoo-kernel
	local p
	for p in [0-9]*.patch; do
		case ${p} in 8001-*|8002-*) continue ;; esac
		doins "${p}"
	done

	# kernel config fragment (merged by the dist-kernel eclass after Gentoo's config)
	insinto /etc/kernel/config.d
	cat "${FILESDIR}/t2.config" > "${T}/90-horcrux-t2.config" || die
	echo "# --- t2linux extra_config (pinned ${T2_COMMIT:0:7})" >> "${T}/90-horcrux-t2.config"
	cat extra_config >> "${T}/90-horcrux-t2.config" || die
	newins "${T}/90-horcrux-t2.config" 90-horcrux-t2.config
	insinto /usr/share/horcrux-t2
	doins "${FILESDIR}/required-config.txt"

	# initramfs: apple-bce + HID early (keyboard at the passphrase prompt), same list as the installer's t2.py
	insinto /etc/dracut.conf.d
	newins - 90-horcrux-t2.conf <<-'EOF'
		# Horcrux T2 profile: the internal keyboard/trackpad of a T2 Mac is a USB device behind the T2 and
		# only appears once apple-bce (t2bce_*) is loaded.
		hostonly="no"
		add_drivers+=" t2bce_core t2bce_dma t2bce_vhci hid hid_generic usbhid hid_apple hid_appletb_kbd hid_appletb_bl hid_magicmouse nvme dm_crypt dm_mod xhci_pci xhci_hcd "
		force_drivers+=" t2bce_core t2bce_dma t2bce_vhci hid_apple hid_appletb_kbd "
		add_dracutmodules+=" crypt dm "
		omit_dracutmodules+=" plymouth "
	EOF
	insinto /etc/modprobe.d
	newins - 90-horcrux-t2.conf <<-'EOF'
		softdep hid_apple pre: t2bce_vhci
		softdep hid_appletb_kbd pre: t2bce_vhci
		softdep t2bce_vhci pre: t2bce_core t2bce_dma
		options hid_appletb_kbd mode=1
	EOF
	insinto /etc/modules-load.d
	newins - horcrux-t2.conf <<-'EOF'
		t2bce_core
		t2bce_dma
		t2bce_vhci
		t2bce_audio
		hid_apple
		hid_appletb_kbd
		hid_appletb_bl
		brcmfmac
		brcmfmac_wcc
		hci_bcm4377
		apple_mfi_fastcharge
	EOF
}

pkg_postinst() {
	elog "Rebuild the kernel so it carries the patches: emerge -1 sys-kernel/gentoo-kernel"
	elog "Kernel command line for a T2 laptop: pm_async=off intel_iommu=on iommu=pt"
}
