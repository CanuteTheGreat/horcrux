# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Horcrux GNOME desktop edition (on top of sys-apps/horcrux-core): AppIndicator tray icons, first-login Wi-Fi chooser, login keyring that really unlocks"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64 arm64"
# awcc: also enable the Alienware control extension (app-misc/awccd). The system db REPLACES the gschema default of the same key (an awccd gschema override
# would be shadowed), so every default extension of the machine is listed here, in the one compiled db.
IUSE="awcc"

# DESKTOP EDITION, layered on the server core: sys-apps/horcrux-core carries everything every Horcrux install needs (chrony + the clock rules +
# the NetworkManager/dhcpcd clock hooks, first-boot network wait, overlay sync config, backup, ZFS dist-kernel rule). Nothing of that is attached to
# this package any more: a headless server gets the same fixes without a single GNOME package. This package only adds the GNOME defaults below.
RDEPEND="
	sys-apps/horcrux-core
	gnome-base/dconf
	sys-apps/dbus
	gnome-base/gnome-control-center
	net-misc/networkmanager
	gnome-extra/gnome-shell-extension-appindicator
	gnome-extra/gnome-shell-extensions
	gnome-base/gnome-keyring[pam]
	app-crypt/libsecret
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

	exeinto /usr/libexec
	doexe "${FILESDIR}"/horcrux-network-chooser
	insinto /etc/xdg/autostart
	doins "${FILESDIR}"/horcrux-network-chooser.desktop

	# login keyring (owner order 2026-11-03: never sign in again after a reinstall/restore/reboot): see the header of horcrux-keyring-pam
	exeinto /usr/libexec
	doexe "${FILESDIR}"/horcrux-keyring-pam
	dobin "${FILESDIR}"/horcrux-keyring-status

	# D-Bus-activated apps (GNOME Settings, Files, portals) must get the session environment: on OpenRC the user dbus-daemon starts before gnome-shell, so
	# its activation environment lacks XDG_CURRENT_DESKTOP & co and gnome-control-center refuses to run. Pushed once per login after the shell is up.
	exeinto /usr/libexec
	doexe "${FILESDIR}"/horcrux-session-env
	insinto /etc/xdg/autostart
	doins "${FILESDIR}"/horcrux-session-env.desktop
}

pkg_postinst() {
	# pambase[gnome-keyring] + gdm-password start TWO pam_gnome_keyring daemons, the first before /run/user/UID exists: no persistent login keyring.
	# Applied here so it holds for the image build, the installer's target world and every installed machine through @world. Takes effect at the
	# NEXT login (no logout needed to install it); idempotent. A later pambase update leaves a pending ._cfg file: do NOT accept its keyring lines.
	"${EROOT}"/usr/libexec/horcrux-keyring-pam fix "${ROOT%/}" || ewarn "horcrux-keyring-pam: the keyring PAM stack is still not right, run: /usr/libexec/horcrux-keyring-pam check"
	elog "Login keyring: log out and in once (or reboot) after this update. Secrets stored in the session before that (Nextcloud/Proton sign-ins)"
	elog "were kept in RAM only: sign in again once afterwards and they persist. Check with: horcrux-keyring-status"
	elog "The keyring unlocks with the login password; if the login password differs from the password of an old (restored) keyring, GNOME asks once to unlock it."
}
