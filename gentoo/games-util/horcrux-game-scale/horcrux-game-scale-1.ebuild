# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

PYTHON_COMPAT=( python3_{11..15} )
inherit python-single-r1

DESCRIPTION="Run games at the right size on a scaled GNOME desktop (nested gamescope wrapper + Steam launch options)"
HOMEPAGE="https://horcrux.errant.solutions/"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64"
REQUIRED_USE="${PYTHON_REQUIRED_USE}"

# mutter >= 50 always scales Xwayland natively, so X11 games draw at 1/ceil(scale) of the desktop's size: the wrapper runs them in
# a nested gamescope (a Wayland client that mutter sizes to the monitor). gamescope is ~amd64 on ::gentoo: its keyword is owned by
# sys-apps/horcrux-repo-config (package.accept_keywords/horcrux-roles), so it resolves before this package is installed.
# Opt-in: nothing in the desktop editions pulls this package; a machine that has Steam installs it (machine profile / layer).
RDEPEND="
	${PYTHON_DEPS}
	$(python_gen_cond_dep 'dev-python/pygobject:3[${PYTHON_USEDEP}]')
	gui-wm/gamescope
"

src_install() {
	python_newscript "${FILESDIR}/horcrux-game-scale" horcrux-game-scale
	python_newscript "${FILESDIR}/horcrux-steam-launch-options" horcrux-steam-launch-options
	insinto /etc/horcrux
	newins "${FILESDIR}/game-scale.toml" game-scale.toml
	# init-neutral: a session autostart entry (GNOME starts it on OpenRC and on systemd alike); it only edits Steam's
	# localconfig.vdf while Steam is not running
	insinto /etc/xdg/autostart
	doins "${FILESDIR}/horcrux-steam-launch-options.desktop"
}

pkg_postinst() {
	elog "Every Steam game gets the launch option 'horcrux-game-scale %command%' the next time Steam is closed while you are"
	elog "logged in (or run: horcrux-steam-launch-options apply). Undo: horcrux-steam-launch-options remove."
	elog "Settings: /etc/horcrux/game-scale.toml (per user: ~/.config/horcrux/game-scale.toml). One game unwrapped: launch option"
	elog "  HORCRUX_GAME_SCALE=0 %command%"
}
