# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

PYTHON_COMPAT=( python3_{13..14} )
inherit python-single-r1

DESCRIPTION="Horcrux tooling: boot-environment manager (horcrux-be), live installer + console menu, T2 firmware importer, ownership audit"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="~amd64"

# horcrux-be: ZFS boot environments (snapshot/rollback/activate), static grub.cfg per BE, kernel-hook sync
REQUIRED_USE="${PYTHON_REQUIRED_USE}"
RDEPEND="
	${PYTHON_DEPS}
	sys-fs/zfs
	sys-boot/grub
"

src_install() {
	# the files/ tree mirrors build/live-iso of the Horcrux repo (sync-files.sh); modes come from git
	cp -a "${FILESDIR}"/usr "${ED}"/ || die
	find "${ED}" -name __pycache__ -prune -exec rm -rf {} + || true
	# byte-compile here so the .pyc files are owned by this package instead of appearing at first run
	python_optimize "${ED}/usr/lib/live-installer"
}
