# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Horcrux GNOME system-wide defaults (dconf system db): AppIndicator tray icons enabled for every user"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64 arm64"
# awcc: also enable the Alienware control extension (app-misc/awccd). The system db REPLACES the gschema default of the same key (an awccd gschema override
# would be shadowed), so every default extension of the machine is listed here, in the one compiled db.
IUSE="awcc"

RDEPEND="
	gnome-base/dconf
	gnome-extra/gnome-shell-extension-appindicator
	gnome-extra/gnome-shell-extensions
"
BDEPEND="gnome-base/dconf"

src_install() {
	# GNOME has no tray without AppIndicator: Nextcloud and friends were invisible. A SYSTEM dconf db (not per-user gsettings), so the default
	# holds for every account; a user's own choice (user-db) still wins. The compiled db is part of the package (dconf compile), no `dconf update` needed.
	insinto /etc/dconf/profile
	newins - user <<-'EOF'
		user-db:user
		system-db:local
	EOF
	insinto /etc/dconf/db/local.d
	newins - 00-horcrux-extensions <<-'EOF'
		[org/gnome/shell]
		enabled-extensions=['appindicatorsupport@rgcjonas.gmail.com'@@AWCC@@]
	EOF
	if use awcc; then
		sed -i "s|@@AWCC@@|, 'awcc@horcrux.errant.solutions'|" "${ED}/etc/dconf/db/local.d/00-horcrux-extensions" || die
	else
		sed -i 's|@@AWCC@@||' "${ED}/etc/dconf/db/local.d/00-horcrux-extensions" || die
	fi
	dconf compile "${T}/local" "${ED}/etc/dconf/db/local.d" || die "dconf compile failed"
	insinto /etc/dconf/db
	doins "${T}/local"
}
