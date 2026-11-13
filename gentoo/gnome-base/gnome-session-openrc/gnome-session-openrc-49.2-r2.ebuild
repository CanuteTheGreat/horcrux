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
IUSE="X"

# -r2 (this overlay, ::gentoo has -r1): everything the Horcrux images used to apply as /etc/portage/patches now ships here, plus the
# session-scoping fix:
#  0001/0002  gsd meta services are not supervised, gsd audio starts after PipeWire
#  0003       gnome-shell stays on the OpenRC user bus (gsd-media-keys can grab keys)
#  0004       the graphical session OWNS its user services. The user runlevel is shared by all logins of the user and only stops with the
#             LAST one, so an ssh/su login kept gnome-shell alive after a GNOME logout and supervise-daemon respawned it into the closed
#             session: it grabbed the seat (mutter EBUSY) and the next login had no mouse and keyboard. Now the leader tears the services
#             down when the session ends, a new session cleans up what an old one left, and gnome-shell only (re)starts while its session
#             is alive (gnome-session-openrc-helper). The helper's activation-env also gives the user bus's D-Bus activation environment
#             the session's XDG_CURRENT_DESKTOP & co. (from gnome-shell's start_pre), so xdg-desktop-portal picks its GNOME backend
#             (ScreenCast / screen sharing). Upstream issue with this patch: https://github.com/swagtoy/gnome-session-openrc/issues/12
COMMON_DEPEND="
	>=dev-libs/glib-2.82.0:2
	>=sys-auth/elogind-242
	sys-apps/openrc
"
RDEPEND="
	${COMMON_DEPEND}
	sys-apps/dbus[X?]
	!<gnome-base/gnome-session-49
"
DEPEND="${COMMON_DEPEND}"
BDEPEND="
	virtual/pkgconfig
"

PATCHES=(
	"${FILESDIR}"/0001-gsd-meta-service-no-supervisor.patch
	"${FILESDIR}"/0002-gsd-audio-after-pipewire.patch
	"${FILESDIR}"/0003-shell-keep-user-bus.patch
	"${FILESDIR}"/0004-session-scoped-user-services.patch
)

# Horcrux images before -r2 carried 0001-0003 as user patches (/etc/portage/patches/gnome-base/gnome-session-openrc/). They live in this ebuild now,
# and eapply_user would apply an existing copy a second time ("Reversed (or previously applied) patch" -> prepare fails). The ebuild never edits the
# admin's /etc/portage: a byte-identical user copy is skipped here and left to eapply_user, and 0004 (made on top of 0001-0003) then follows the user
# patches. Any other user patch is applied as usual.
_hx_user_copy() {
	local d f
	for d in "${PORTAGE_CONFIGROOT%/}/etc/portage/patches/${CATEGORY}"/{${P}-${PR},${P},${PN}}{,:${SLOT%/*}}; do
		f="${d}/${1##*/}"
		[[ -f ${f} ]] && cmp -s "${1}" "${f}" && return 0
	done
	return 1
}

src_prepare() {
	local p dup=0 late=()
	for p in "${PATCHES[@]}"; do
		if _hx_user_copy "${p}"; then
			einfo "${p##*/}: identical copy in /etc/portage/patches is applied by eapply_user (this ebuild ships it, the copy can go)"
			dup=1
		elif [[ ${p##*/} == 0004-* && ${dup} == 1 ]]; then
			late+=( "${p}" )
		else
			eapply "${p}"
		fi
	done
	eapply_user
	[[ ${#late[@]} -gt 0 ]] && eapply "${late[@]}"
	# the session-scoping helper must be in place (the image build gates and the QEMU lifecycle test look for it)
	[[ -f data/gnome-session-openrc-helper ]] || die "0004-session-scoped-user-services.patch not applied"
}

src_configure() {
	local emesonargs=(
		$(meson_use X x11)
	)
	meson_src_configure
}
