# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="PRIVATE gringotts inputs: the agent's SSH public keys and the 7-day ephemeral tagged Tailscale pre-auth key"
HOMEPAGE="https://git.canutethegreat.com/CanuteTheGreat/horcrux"
# Never published: the tarball is produced outside the repository (private bundle) and dropped into DISTDIR by the image build.
SRC_URI="gringotts-private-${PV}.tar"
S="${WORKDIR}"

LICENSE="all-rights-reserved"
SLOT="0"
KEYWORDS="amd64"
RESTRICT="fetch mirror bindist strip"

pkg_nofetch() {
	einfo "Put gringotts-private-${PV}.tar (authorized_keys + ts-authkey) into ${DISTDIR}; it is not distributable."
}

src_install() {
	insinto /etc/gringotts
	doins authorized_keys
	insinto /usr/share/gringotts
	insopts -m0600
	doins ts-authkey
}
