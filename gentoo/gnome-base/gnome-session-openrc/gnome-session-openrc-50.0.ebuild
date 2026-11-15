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
#    Upstream issue with this patch: https://github.com/swagtoy/gnome-session-openrc/issues/12
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
)

src_prepare() {
	default
	# the session-scoping helper must be in place (the image build gates and the QEMU lifecycle test look for it)
	[[ -f data/gnome-session-openrc-helper ]] || die "session-scoping patch not applied"
}
