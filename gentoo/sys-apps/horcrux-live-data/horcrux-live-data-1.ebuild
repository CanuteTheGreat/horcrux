# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Data files of the Horcrux installer: branding images and the Portage metadata snapshot for the USE-flag page"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
# Made by build/live-iso (variants/hogshead/Dockerfile.part2) from /usr/share/live-installer of the image being built:
# the branding is in the repo, portage-meta.json.xz is generated from that image's Portage tree (mk-portage-meta.py), so there
# is no public source to fetch. A new image build makes a new tarball.
SRC_URI="${P}.tar.zst"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="~amd64"
RESTRICT="fetch mirror bindist"

BDEPEND="app-arch/zstd"
RDEPEND="sys-apps/horcrux-tools"

pkg_nofetch() {
	einfo "${SRC_URI} is made by build/live-iso/project/variants/hogshead/Dockerfile.part2 (tar of /usr/share/live-installer)."
}

src_unpack() {
	tar --zstd -xf "${DISTDIR}/${SRC_URI}" || die
}

src_install() {
	insinto /usr/share
	doins -r usr/share/live-installer
}
