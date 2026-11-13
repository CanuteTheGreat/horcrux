# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

inherit desktop

DESCRIPTION="Horcrux os-release: name, version, build id and variant of an installed Horcrux system (replaces baselayout's /usr/lib/os-release)"
HOMEPAGE="https://horcrux.errant.solutions"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64 arm64"

# No variant flag = the headless Server. The image layer (project/variants/<v>/portage/package.use/horcrux-release) or the machine's own
# package.use sets the edition (gnome-light) and at most one machine layer; the flags are part of the binary package, so a cached binpkg of
# another variant is never reused.
IUSE="gnome-light hogsmeade hogshead gringotts surface test-machine"
REQUIRED_USE="?? ( hogsmeade hogshead gringotts surface test-machine )"

RDEPEND="sys-apps/baselayout"

# VERSION/VERSION_ID/BUILD_ID come from PV: the release ebuild is cut together with every horcrux-bin ebuild (bump-horcrux-bin.sh), so an
# upgrade (emerge -u) of the manager moves them.
#
# Gentoo ships /usr/lib/os-release in sys-apps/baselayout. The proper way to own it from another package: baselayout is merged with
# INSTALL_MASK=/usr/lib/os-release (package.env, installed below) so it never installs its copy, and this package installs the file.
# /etc/os-release stays baselayout's symlink to ../usr/lib/os-release.
# First install over an existing baselayout: Portage's protect-owned refuses a file another package owns, and the package.env above only
# applies from the NEXT emerge run. pkg_setup moves baselayout's copy aside (a path that does not exist cannot collide); pkg_postinst
# drops it. baselayout's database entry for it goes away at its next merge (the mask is then in place).
pkg_setup() {
	[[ -n ${REPLACING_VERSIONS} ]] && return
	local f=${EROOT}/usr/lib/os-release
	if [[ -f ${f} && ! -L ${f} ]] && grep -qs '^obj /usr/lib/os-release ' "${EROOT}"/var/db/pkg/sys-apps/baselayout-*/CONTENTS; then
		einfo "moving baselayout's ${f} aside: this package owns it"
		mv -f "${f}" "${f}.baselayout" || die
	fi
}

src_install() {
	local edition=server m machine=""
	use gnome-light && edition=gnome-light
	for m in hogsmeade hogshead gringotts surface test-machine; do use ${m} && machine=${m}; done

	bash "${FILESDIR}"/render-os-release.sh "${PV}" "${edition}" "${machine}" > "${T}"/os-release || die
	insinto /usr/lib
	doins "${T}"/os-release

	local s
	for s in 64 128 256 512; do
		newicon -s ${s} "${FILESDIR}"/icons/horcrux-logo-${s}.png horcrux-logo.png
	done

	insinto /etc/portage/env
	doins "${FILESDIR}"/horcrux-baselayout.conf
	insinto /etc/portage/package.env
	newins "${FILESDIR}"/package.env horcrux-release
}

pkg_postinst() {
	rm -f "${EROOT}"/usr/lib/os-release.baselayout
}

pkg_postrm() {
	[[ -z ${REPLACING_VERSIONS} ]] && ewarn "baselayout's os-release is masked: re-emerge sys-apps/baselayout without ${CATEGORY}/${PN}'s package.env to get it back"
}
