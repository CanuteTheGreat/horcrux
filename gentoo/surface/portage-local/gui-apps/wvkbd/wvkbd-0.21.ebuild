# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

inherit toolchain-funcs

DESCRIPTION="On-screen keyboard for wlroots compositors (sway/labwc), virtual-keyboard protocol"
HOMEPAGE="https://github.com/jjsullivan5196/wvkbd"
SRC_URI="https://github.com/jjsullivan5196/wvkbd/archive/refs/tags/v${PV}.tar.gz -> ${P}.tar.gz"

LICENSE="GPL-3+ MIT"
SLOT="0"
KEYWORDS="amd64 arm64"

DEPEND="
	dev-libs/wayland
	x11-libs/libxkbcommon
	x11-libs/pango
	x11-libs/cairo
"
RDEPEND="${DEPEND}"
BDEPEND="
	dev-util/wayland-scanner
	virtual/pkgconfig
	app-text/scdoc
"

src_compile() {
	emake CC="$(tc-getCC)" LAYOUT=mobintl PREFIX=/usr
}

src_install() {
	emake DESTDIR="${D}" PREFIX=/usr MANPREFIX=/usr/share/man LAYOUT=mobintl install
}
