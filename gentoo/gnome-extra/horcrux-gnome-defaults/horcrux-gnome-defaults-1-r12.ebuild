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
IUSE="awcc systemd"

# -r12: horcrux-keyring-pam also makes sure /etc/pam.d/passwd re-keys the login keyring (the systemd profile's passwd only includes system-auth).
# -r11: GNOME on OpenRC comes from gnome-base/gnome-session-openrc-49.2-r2::horcrux (the gsd/PipeWire/user-bus fixes that were /etc/portage/patches in
# the images, plus session-scoped user services: a GNOME logout stops the session's gnome-shell & co. even while an ssh login of the user keeps the OpenRC
# user runlevel alive, and no supervise-daemon respawn into a closed session). ::gentoo's copies are masked here (package.mask/horcrux-gnome-session-openrc),
# so an `emerge -u` can never swap the fixed package for an unpatched newer ::gentoo version; this overlay carries each version we ship.
# -r10: x11-themes/gentoo-xcursors (::gentoo) as an extra cursor theme on every desktop edition; Adwaita stays the default, nothing selects it.
# -r8: appindicator >= 66-r1 (this overlay): Chromium/Electron 43+ tray icons (Slack, Signal, Docker Desktop) are dropped by ::gentoo's 64/66
# ('error occurred in Get' on the unique bus name); see gnome-shell-extension-appindicator-66-r1.ebuild and the sni-chromium gate.
# DESKTOP EDITION, layered on the server core: sys-apps/horcrux-core carries everything every Horcrux install needs (chrony + the clock rules +
# the NetworkManager/dhcpcd clock hooks, first-boot network wait, overlay sync config, backup, ZFS dist-kernel rule). Nothing of that is attached to
# this package any more: a headless server gets the same fixes without a single GNOME package. This package only adds the GNOME defaults below.
RDEPEND="
	sys-apps/horcrux-core
	gnome-base/dconf
	sys-apps/dbus
	gnome-base/gnome-control-center
	net-misc/networkmanager
	>=gnome-extra/gnome-shell-extension-appindicator-66-r1
	gnome-extra/gnome-shell-extensions
	gnome-base/gnome-keyring[pam]
	app-crypt/libsecret
	x11-themes/gentoo-xcursors
	!systemd? ( >=gnome-base/gnome-session-openrc-49.2-r2 )
"
BDEPEND="gnome-base/dconf"

src_install() {
	insinto /etc/portage/package.mask
	newins - horcrux-gnome-session-openrc <<-'EOF4'
		# owned by gnome-extra/horcrux-gnome-defaults: GNOME on OpenRC is gnome-base/gnome-session-openrc::horcrux (session-scoped user services,
		# gsd/PipeWire/user-bus fixes); an unpatched ::gentoo version must never replace it on update
		gnome-base/gnome-session-openrc::gentoo
	EOF4

	# the Horcrux os-release (sys-apps/horcrux-release) names this layer: its flags are read from package.use. Shipped here, so the
	# installed system keeps its variant on every update (a USE dependency would stop `emerge -u` on a host that lacks the flags).
	insinto /etc/portage/package.use
	newins - horcrux-release-edition <<-EOF3
		# owned by this package
		sys-apps/horcrux-release gnome-light
	EOF3
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
	# Fractional display scaling (125%/150%/175%...) in Settings > Displays: stock mutter offers only 100%/200%. scale-monitor-framebuffer is the Wayland
	# fractional-scale switch. xwayland-native-scaling is deliberately NOT enabled: it hands X11 apps unscaled native pixels and each must scale itself, and
	# most (Electron --ozone-platform=x11, Java, Qt/GTK3 X11) do not, so they render tiny; without it mutter scales XWayland clients (right size, slightly soft).
	# A default, not a lock: `gsettings set` by a user still wins. Wayland-only (mutter's X11 backend ignores it), so the GNOME-on-Xorg session is unaffected.
	newins - 10-horcrux-fractional-scaling <<-'EOF'
		[org/gnome/mutter]
		experimental-features=['scale-monitor-framebuffer']
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
