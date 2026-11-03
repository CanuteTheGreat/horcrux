# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Portage repos.conf entry that syncs the Horcrux overlay from its own git repository"
HOMEPAGE="https://git.canutethegreat.com/CanuteTheGreat/horcrux-overlay"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64 arm64"

RDEPEND="dev-vcs/git"

src_install() {
	insinto /etc/portage/repos.conf
	doins "${FILESDIR}/horcrux.conf"
}

pkg_postinst() {
	if [[ -d ${EROOT}/var/db/repos/horcrux && ! -d ${EROOT}/var/db/repos/horcrux/.git ]]; then
		ewarn "${EROOT}/var/db/repos/horcrux is a frozen copy from the installer, not a git checkout. Portage refuses to clone into it:"
		ewarn "  mv /var/db/repos/horcrux /var/db/repos/horcrux.frozen && emaint sync -r horcrux && rm -rf /var/db/repos/horcrux.frozen"
	fi
}
