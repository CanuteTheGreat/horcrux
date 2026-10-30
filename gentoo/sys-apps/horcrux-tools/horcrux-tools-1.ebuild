# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Horcrux boot-environment manager (horcrux-be) and Apple T2 firmware importer"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="~amd64"

# horcrux-be: ZFS boot environments (snapshot/rollback/activate), static grub.cfg per BE, kernel-hook sync
RDEPEND="
	dev-lang/python
	sys-fs/zfs
	sys-boot/grub
"

src_install() {
	dobin "${FILESDIR}"/horcrux-be "${FILESDIR}"/horcrux-t2-firmware
	# the two modules the tools import (be.py, t2.py); the rest of the live installer is not part of this package
	insinto /usr/lib/live-installer/liveinst
	doins "${FILESDIR}"/liveinst/{__init__,be,t2}.py
}
