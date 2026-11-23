# Copyright 1999-2026 Gentoo Authors
# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8
inherit meson

DESCRIPTION="Gnome session leader for OpenRC (Horcrux: session-scoped user services)"
HOMEPAGE="https://github.com/swagtoy/gnome-session-openrc https://github.com/CanuteTheGreat/horcrux"
SRC_URI="https://github.com/swagtoy/${PN}/archive/v${PV}.tar.gz -> ${P}.tar.gz"

LICENSE="GPL-2+"
SLOT="0"
KEYWORDS="~alpha amd64 ~arm arm64 ~loong ~ppc ~ppc64 ~riscv ~sparc x86"

# 50.0 (this overlay): upstream 50 is Wayland-only (the -x11/-wayland service variants and the X USE flag are gone). Everything the Horcrux
# images need ships as ONE patch on top of upstream (the 49.2 series 0001-0004 ported to the 50 layout):
#  - gsd meta services are not supervised, gsd audio starts after PipeWire (gsd-media-keys/gsd-sound want pipewire-pulse + wireplumber)
#  - gnome-shell stays on the OpenRC user bus (gsd-media-keys can grab keys)
#  - the graphical session OWNS its user services. The user runlevel is shared by all logins of the user and only stops with the LAST one,
#    so an ssh/su login kept gnome-shell alive after a GNOME logout and supervise-daemon respawned it into the closed session: it grabbed
#    the seat (mutter EBUSY) and the next login had no mouse and keyboard. Now the leader tears the services down when the session ends,
#    a new session cleans up what an old one left, and gnome-shell only (re)starts while its session is alive
#    (gnome-session-openrc-helper). The helper's activation-env also gives the user bus's D-Bus activation environment the session's
#    XDG_CURRENT_DESKTOP & co., so xdg-desktop-portal picks its GNOME backend (ScreenCast / screen sharing).
#  - gnome-session-service (an OpenRC user service) starts with the environment of the user's supervisor, which has no desktop identity, and
#    hands it to every app it autostarts: NotShowIn=GNOME entries (nm-applet) started anyway and apps picked non-GNOME defaults (grey
#    Nextcloud tray icon). gnome-session-service-wrapper now loads XDG_CURRENT_DESKTOP, XDG_SESSION_DESKTOP, DESKTOP_SESSION & co. from the
#    session leader first (gnome-session-openrc-helper session-env). -r1.
#  - -r2: GDM 50 launches the login screen as plain `gnome-session` of a greeter-class session and gnome-session's own main() sets
#    GNOME_SHELL_SESSION_MODE=gdm; this OpenRC leader bypasses it, so gnome-shell came up as an empty desktop of gdm-greeter (no login
#    dialog, nobody could log in). gnome-shell's start_pre now sets the mode for greeter-class sessions (0002).
#    Upstream issue with this patch: https://github.com/swagtoy/gnome-session-openrc/issues/12
#  - -r3: X11 apps were tiny on a fractionally scaled monitor (Steam, Qt apps such as the Nextcloud client, terminator). mutter 50 always
#    runs Xwayland natively scaled (X screen = ceil(scale) x the logical size, X11 windows scaled down by the compositor), so an X11 app
#    renders at the right size only when gsd-xsettings publishes the factor (XSETTINGS Gdk/WindowScalingFactor, Xft.dpi = 96 x factor).
#    Under systemd gnome-session starts gsd-xsettings with Xwayland; upstream OpenRC never started it (commented out of
#    gnome-settings-daemon, DISPLAY=:2 hard-coded). Now mutter's /etc/xdg/Xwayland-session.d hook records the display it started and
#    starts gsd-xsettings for it (guarded like gnome-shell, torn down with the session); `gnome-session-openrc-helper x11-scale-status`
#    checks it (0003). Per-app scale variables (GDK_SCALE, QT_SCALE_FACTOR, STEAM_FORCE_DESKTOPUI_SCALING) are deliberately NOT set:
#    they are fixed at app start, ignore scale changes and double-scale on top of Xft.dpi.
COMMON_DEPEND="
	>=dev-libs/glib-2.82.0:2
	>=sys-auth/elogind-242
	sys-apps/openrc
"
RDEPEND="
	${COMMON_DEPEND}
	sys-apps/dbus
	!<gnome-base/gnome-session-49
"
DEPEND="${COMMON_DEPEND}"
BDEPEND="
	virtual/pkgconfig
"

PATCHES=(
	"${FILESDIR}"/0001-horcrux-session-scoped-user-services.patch
	"${FILESDIR}"/0002-greeter-shell-session-mode.patch
	"${FILESDIR}"/0003-xwayland-gsd-xsettings.patch
)

src_prepare() {
	default
	# the session-scoping helper must be in place (the image build gates and the QEMU lifecycle test look for it)
	[[ -f data/gnome-session-openrc-helper ]] || die "session-scoping patch not applied"
	[[ -f data/Xwayland-session.d/20-gnome-session-openrc-xsettings ]] || die "Xwayland gsd-xsettings patch not applied"
	! grep -q '^DISPLAY=:' data/openrc/gsd-xsettings || die "gsd-xsettings still has a hard-coded DISPLAY"
}

pkg_postinst() {
	# a running session started its Xwayland before this hook existed: X11 apps get the UI scale from the next login (or start
	# gsd-xsettings in the session: rc-service -U gsd-xsettings start), then restart the X11 app
	elog "X11 apps get the GNOME UI scale through gsd-xsettings from the next login on."
}
