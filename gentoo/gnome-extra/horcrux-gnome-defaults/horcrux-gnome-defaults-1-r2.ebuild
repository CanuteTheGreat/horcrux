# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Horcrux GNOME system-wide defaults: AppIndicator tray icons, clock sync hook, first-login Wi-Fi chooser, login keyring that really unlocks"
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
	sys-apps/horcrux-repo-config
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
		enabled-extensions=['appindicatorsupport@rgcjonas.gmail.com']
	EOF
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
}

pkg_postinst() {
	# pambase[gnome-keyring] + gdm-password start TWO pam_gnome_keyring daemons, the first before /run/user/UID exists: no persistent login keyring.
	# Applied here so it holds for the image build, the installer's target world and every installed machine through @world. Takes effect at the
	# NEXT login (no logout needed to install it); idempotent. A later pambase update leaves a pending ._cfg file: do NOT accept its keyring lines.
	"${EROOT}"/usr/libexec/horcrux-keyring-pam fix "${ROOT%/}" || ewarn "horcrux-keyring-pam: the keyring PAM stack is still not right, run: /usr/libexec/horcrux-keyring-pam check"
	elog "Login keyring: log out and in once (or reboot) after this update. Secrets stored in the session before that (Brave/Nextcloud/Proton sign-ins)"
	elog "were kept in RAM only: sign in again once afterwards and they persist. Check with: horcrux-keyring-status"
	elog "The keyring unlocks with the login password; if the login password differs from the password of an old (restored) keyring, GNOME asks once to unlock it."
}
