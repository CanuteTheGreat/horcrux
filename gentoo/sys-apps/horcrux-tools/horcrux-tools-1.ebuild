# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

PYTHON_COMPAT=( python3_{13..14} )
inherit python-single-r1

DESCRIPTION="Horcrux tooling: boot-environment manager (horcrux-be), live installer + console menu, T2 firmware importer, ownership audit"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
# The tarball is made by build/live-iso (variants/hogshead/Dockerfile.part2) from the repository tree at image-build time
# (common/overlay installer + scripts, desktop service list, horcrux-orphans): nothing is downloaded. These tools track the image
# they ship in, so a new image build makes a new tarball.
SRC_URI="${P}.tar.zst"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="~amd64"
RESTRICT="fetch mirror bindist"

REQUIRED_USE="${PYTHON_REQUIRED_USE}"
# horcrux-be: ZFS boot environments (snapshot/rollback/activate), static grub.cfg per BE, kernel-hook sync
RDEPEND="
	${PYTHON_DEPS}
	sys-fs/zfs
	sys-boot/grub
"
BDEPEND="app-arch/zstd"

pkg_nofetch() {
	einfo "${SRC_URI} is made by build/live-iso/project/variants/hogshead/Dockerfile.part2 (tar of the installer scripts)."
}

src_unpack() {
	tar --zstd -xf "${DISTDIR}/${SRC_URI}" || die
}

src_install() {
	exeinto /usr/bin
	doexe usr/bin/*
	exeinto /usr/libexec
	doexe usr/libexec/*
	insinto /usr/lib
	doins -r usr/lib/live-installer
	# the desktop service list is optional: a device package (Surface) ships its own under the same path
	if [[ -d usr/share/horcrux/desktop ]]; then
		insinto /usr/share/horcrux
		doins -r usr/share/horcrux/desktop
	fi
	# optional /etc payload (live-ISO init services, console-menu profile hook, zram-swap, installer profile): only variants whose tarball has
	# an etc/ tree get it (the Surface image); the hogshead tarball has none. cp -a keeps the init scripts executable.
	if [[ -d etc ]]; then cp -a etc "${ED}/" || die; fi
	# byte-compile here so the .pyc files are owned by this package instead of appearing at first run
	python_optimize "${ED}/usr/lib/live-installer"
}
