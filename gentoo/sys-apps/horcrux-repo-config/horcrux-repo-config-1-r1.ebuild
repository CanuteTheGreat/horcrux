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
	# keywords the Horcrux role packages need (testing on ::gentoo). They live HERE, not in the role package: Portage reads package.accept_keywords while it
	# resolves the role package's own dependencies, so a file the role package would install is too late for the first install.
	insinto /etc/portage/package.accept_keywords
	newins "${FILESDIR}/horcrux-roles.keywords" horcrux-roles
	insinto /etc/portage/package.use
	newins "${FILESDIR}/horcrux-roles.use" horcrux-roles
}

pkg_postinst() {
	if [[ -d ${EROOT}/var/db/repos/horcrux && ! -d ${EROOT}/var/db/repos/horcrux/.git ]]; then
		ewarn "${EROOT}/var/db/repos/horcrux is a frozen copy from the installer, not a git checkout. Portage refuses to clone into it:"
		ewarn "  mv /var/db/repos/horcrux /var/db/repos/horcrux.frozen && emaint sync -r horcrux && rm -rf /var/db/repos/horcrux.frozen"
	fi
}
