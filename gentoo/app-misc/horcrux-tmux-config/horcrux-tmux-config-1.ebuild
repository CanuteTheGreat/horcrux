# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Horcrux desktop: tmux copies mouse selections to the desktop clipboard (system /etc/tmux.conf + wl-copy/xclip helper)"
HOMEPAGE="https://horcrux.errant.solutions/"
S="${WORKDIR}"

LICENSE="GPL-2"
SLOT="0"
KEYWORDS="amd64 arm64"

# Pulled by every DESKTOP edition meta (horcrux-gnome-defaults, horcrux-surface-desktop), never by the server core. With `mouse on` tmux owns the
# drag and, without this, copies only into its own buffer (VTE terminals have no OSC 52, no clipboard tool was installed).
RDEPEND="
	app-misc/tmux
	gui-apps/wl-clipboard
	x11-misc/xclip
"

src_install() {
	insinto /etc
	newins "${FILESDIR}"/tmux.conf tmux.conf
	dobin "${FILESDIR}"/horcrux-clip-copy
}
