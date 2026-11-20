# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Portage repos.conf entry that syncs the Horcrux overlay from its public GitHub mirror"
HOMEPAGE="https://horcrux.errant.solutions/"
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
	# Mutter's ScreenCast frame limiter capped every monitor stream at 30-40 fps on a 60 Hz display (remote desktop, screen
	# sharing): backport of the upstream fix (mutter >= 51.2). Portage applies it to every source build of x11-wm/mutter;
	# binary packages ignore it, so images and `make.conf` carry --usepkg-exclude=x11-wm/mutter. Drop it once mutter >= 51.2 is stable.
	insinto /etc/portage/patches/x11-wm/mutter
	newins "${FILESDIR}/mutter-screencast-nearest-interval.patch" 0001-screencast-round-to-nearest-interval.patch
}

pkg_postinst() {
	if has_version "<x11-wm/mutter-51.2"; then
		elog "The Mutter ScreenCast frame-limiter patch is installed. It takes effect when x11-wm/mutter is next built from source:"
		elog "  emerge -1 --usepkg-exclude=x11-wm/mutter x11-wm/mutter   (then log out and back in)"
	fi
	if [[ -d ${EROOT}/var/db/repos/horcrux && ! -d ${EROOT}/var/db/repos/horcrux/.git ]]; then
		ewarn "${EROOT}/var/db/repos/horcrux is a frozen copy from the installer, not a git checkout. Portage refuses to clone into it:"
		ewarn "  mv /var/db/repos/horcrux /var/db/repos/horcrux.frozen && emaint sync -r horcrux && rm -rf /var/db/repos/horcrux.frozen"
	fi
}
