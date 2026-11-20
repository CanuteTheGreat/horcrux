# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

inherit systemd

# t2linux 6.18 LTS branch (KVER 6.18.56; Gentoo dropped every 7.2.x kernel and OpenZFS 2.3.x stops at 7.2)
T2_COMMIT="bf06511b536a75fcea3146aa341f183c2a2daebc"

DESCRIPTION="Apple T2 Mac support for sys-kernel/gentoo-kernel: t2linux patch set, kernel config, initramfs config"
HOMEPAGE="https://github.com/t2linux/linux-t2-patches https://wiki.t2linux.org"
SRC_URI="https://github.com/t2linux/linux-t2-patches/archive/${T2_COMMIT}.tar.gz -> linux-t2-patches-${T2_COMMIT}.tar.gz"
S="${WORKDIR}/linux-t2-patches-${T2_COMMIT}"

# the patches are GPL-2 kernel patches
LICENSE="GPL-2"
SLOT="0"
KEYWORDS="~amd64"

# Patch set pinned to t2linux's 6.18 LTS branch: it applies to sys-kernel/gentoo-kernel 6.18.x (Gentoo's long-term
# kernel; OpenZFS 2.3.x supports it). Portage's user-patch mechanism applies it, so every 6.18.y update rebuilds with it.
# Deliberately NO dependency on the kernel (it would pull a ~1 h source build into every config update). The patch set
# does not apply to 6.19+/7.x, so the package ships a mask (/etc/portage/package.mask/horcrux-t2-kernel) that keeps
# gentoo-kernel, gentoo-kernel-bin and virtual/dist-kernel on the 6.18 LTS series.
RDEPEND="
	sys-kernel/dracut
	dev-lang/python
"

src_install() {
	# t2linux patches (APFS 8001/8002 skipped: macOS is gone, out-of-tree driver)
	insinto /etc/portage/patches/sys-kernel/gentoo-kernel
	local p
	for p in [0-9]*.patch; do
		case ${p} in 8001-*|8002-*) continue ;; esac
		doins "${p}"
	done

	# kernel config fragment (merged by the dist-kernel eclass after Gentoo's config)
	# keep the kernel on the series this patch set targets (6.18 LTS)
	insinto /etc/portage/package.mask
	newins - horcrux-t2-kernel <<-'EOF'
		# sys-kernel/horcrux-t2-config: the t2linux patch set targets the 6.18 LTS series (OpenZFS 2.3.x supports it)
		>=sys-kernel/gentoo-kernel-6.19
		>=sys-kernel/gentoo-kernel-bin-6.19
		>=virtual/dist-kernel-6.19
	EOF

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
		# fallback keyboard mode (USB config 1, no tiny-dfr): 2 = media/special keys by default, Fn switches to F1-F12
		options hid_appletb_kbd mode=2 fntoggle=1
	EOF
	# Touch Bar graphical mode (tiny-dfr): attach the display before the daemon starts (see the init script)
	exeinto /usr/libexec
	doexe "${FILESDIR}/horcrux-touchbar"
	newinitd "${FILESDIR}/horcrux-touchbar.initd" horcrux-touchbar
	systemd_dounit "${FILESDIR}/horcrux-touchbar.service"
	insinto /etc/conf.d
	newins - tiny-dfr <<-'EOF'
		# Horcrux T2: the Touch Bar display must be attached (config 2 + USB reset) before tiny-dfr starts
		rc_need="horcrux-touchbar"
	EOF

	# graphical mode: media/special keys by default, F1-F12 while Fn is held (hid_appletb_kbd above only applies to config 1)
	insinto /etc/tiny-dfr
	newins - config.toml <<-'EOF'
		# Horcrux T2: media keys (brightness, volume, play) by default, F1-F12 while Fn is held
		MediaLayerDefault = true
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
