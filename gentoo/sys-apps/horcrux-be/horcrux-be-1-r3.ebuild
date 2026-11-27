# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

PYTHON_COMPAT=( python3_{13..14} )
inherit python-single-r1 systemd

DESCRIPTION="Horcrux boot environments: snapshots/rollback, one boot-menu entry per kernel kept, trial boot of new kernels with fallback to the last known-good one"
HOMEPAGE="https://horcrux.errant.solutions/"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64 arm64"
IUSE="systemd"
REQUIRED_USE="${PYTHON_REQUIRED_USE}"

# WHY THIS IS ITS OWN PACKAGE (2026-11-23, "a bad new kernel has no one-key rollback"): horcrux-be used to ship only inside sys-apps/horcrux-tools, a
# fetch-restricted tarball made at image-build time, so an installed machine could never receive a fix to it (the daily @world update has nothing to fetch).
# This package carries the same program from the repository: files/be.py is byte-identical to
# build/live-iso/common/overlay/usr/lib/live-installer/liveinst/be.py (the copy the installer runs); tests/test_be_kernels.py fails when they differ.
# horcrux-tools no longer installs /usr/bin/horcrux-be (it depends on this package); an install that still has the old wrapper from it loses that file in
# pkg_setup, because Portage refuses to merge over a file another package owns.
# Kernel safety: every boot environment keeps KERNELS_KEEP (default 2) kernels in /boot/be/<be>/ -> one GRUB entry each ("previous kernel"), a new kernel
# is booted ONCE (grub next_entry) and becomes the default only after the boot-time service horcrux-be-good ran in that boot. Settings:
# /etc/horcrux/be.conf (KERNELS_KEEP, KERNEL_TRIAL, GOOD_DELAY). OpenRC script and systemd unit both ship; the program never reboots.
RDEPEND="
	${PYTHON_DEPS}
	net-misc/rsync
	sys-boot/grub
	sys-fs/zfs
"

src_install() {
	python_moduleinto /usr/lib/horcrux-be
	python_domodule "${FILESDIR}"/be.py
	python_doscript "${FILESDIR}"/horcrux-be
	# -r3: the mirror copy of /boot to the extra drives (kernel postinst hook 95-horcrux-boot-sync). It was only in the image-time horcrux-tools
	# tarball, so a fix never reached an installed machine; files/horcrux-boot-sync is byte-identical to
	# build/live-iso/common/overlay/usr/bin/horcrux-boot-sync (tests/test_boot_sync_space.py).
	dobin "${FILESDIR}"/horcrux-boot-sync
	if use systemd; then
		systemd_dounit "${FILESDIR}"/horcrux-be-good.service
	else
		newinitd "${FILESDIR}"/horcrux-be-good.initd horcrux-be-good
	fi
}

pkg_setup() {
	python-single-r1_pkg_setup
	# upgrade from the horcrux-tools-owned wrapper (see above); only on the live system, only when that package owns it
	local f
	for f in horcrux-be horcrux-boot-sync; do
		if [[ -z ${ROOT} && -e /usr/bin/${f} ]] && command -v qfile >/dev/null 2>&1; then
			if [[ $(qfile -qC /usr/bin/${f} 2>/dev/null) == sys-apps/horcrux-tools ]]; then
				rm -f /usr/bin/${f} && einfo "removed the horcrux-tools copy of /usr/bin/${f} (this package owns it now)"
			fi
		fi
	done
}

pkg_postinst() {
	# the boot-time "this kernel is good" step must be on, or a trial kernel would never be promoted (horcrux-be then keeps the newest as default)
	if [[ -e ${EROOT}/etc/horcrux/be.conf || -z ${ROOT} ]]; then
		if use systemd; then
			systemd_enable_service multi-user.target horcrux-be-good.service
		elif command -v rc-update >/dev/null 2>&1; then
			rc-update add horcrux-be-good default >/dev/null 2>&1 || ewarn "rc-update add horcrux-be-good default failed: run it by hand"
		fi
	fi
	elog "Previous kernels stay bootable from the menu (horcrux-be kernels); a new kernel boots once as a trial. horcrux-be boot-previous = previous kernel next boot."
	# a fixed menu generator only helps once the menu is rewritten: regenerate /boot/grub/grub.cfg on the live system (never on a ROOT= image
	# build, never a reboot). -r1: the menu always boots unattended (entry-number fallback chain, retry + restart entry, guarded environment block).
	if [[ -z ${ROOT} && -e /etc/horcrux/be.conf && -d /boot/be ]] && mountpoint -q /boot 2>/dev/null; then
		"${EPREFIX}"/usr/bin/horcrux-be grubcfg >/dev/null && einfo "boot menu regenerated (/boot/grub/grub.cfg)" \
			|| ewarn "horcrux-be grubcfg failed: run it by hand"
	fi
}
