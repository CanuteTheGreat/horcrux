# Copyright 2026 Errant Solutions
# Local ebuild: Cage (Wayland kiosk compositor) is not in the Gentoo tree.
EAPI=8
inherit meson

DESCRIPTION="Wayland kiosk: runs a single application fullscreen (live installer GUI)"
HOMEPAGE="https://github.com/cage-kiosk/cage"
SRC_URI="https://github.com/cage-kiosk/cage/archive/refs/tags/v${PV}.tar.gz -> ${P}.tar.gz"

LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64 arm64 riscv"

RDEPEND="
	dev-libs/wayland
	gui-libs/wlroots:0.19[session]
	x11-libs/libxkbcommon
"
DEPEND="${RDEPEND}
	dev-libs/wayland-protocols
"
BDEPEND="
	dev-util/wayland-scanner
	virtual/pkgconfig
"
