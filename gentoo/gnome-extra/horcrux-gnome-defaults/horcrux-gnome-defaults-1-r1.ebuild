# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Horcrux GNOME system-wide defaults: AppIndicator tray icons, clock sync hook, first-login Wi-Fi chooser"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64 arm64"

# net-misc/horcrux-chrony-nm is in EVERY GNOME install through this package (hogshead had it in its own variant list only; gringotts then booted with a
# months-off clock and no chrony sources): NetworkManager dispatcher hook that makes chronyd re-resolve its sources on link-up.
RDEPEND="
	gnome-base/dconf
	gnome-base/gnome-control-center
	net-misc/chrony
	net-misc/horcrux-chrony-nm
	net-misc/networkmanager
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
		enabled-extensions=['appindicatorsupport@rgcjonas.gmail.com']
	EOF
	dconf compile "${T}/local" "${ED}/etc/dconf/db/local.d" || die "dconf compile failed"
	insinto /etc/dconf/db
	doins "${T}/local"

	exeinto /usr/libexec
	doexe "${FILESDIR}"/horcrux-network-chooser
	insinto /etc/xdg/autostart
	doins "${FILESDIR}"/horcrux-network-chooser.desktop
}
